-- Native-client loading view. Progress represents completed preparation work;
-- server transfer and unknown view readiness never receive a timer percentage.
local M={}
local labels={connecting="Connecting to your match",travel="Loading the arena",waiting="Waiting for the match",
    assets="Loading player assets",present="Creating player models",character="Preparing player characters",equipment="Restoring player equipment",
    controls="Preparing your controls",check_equipment="Checking player equipment",sync="Synchronizing the latest match state",
    view="Preparing your view",error="Unable to load the match"}
local preparation={present=true,character=true,equipment=true,controls=true,check_equipment=true,sync=true}
function M.new(env)
    local self={host=nil,stage="connecting",done=nil,total=nil,failed=false,ready=false,retry_at=0,failures=0,
        started_at=env.now(),paint_at=0,force_update=true}
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
        self.last_error=reason;self.failures=self.failures+1;self.retry_at=env.now()+1;self:fail(reason)
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
            place(w,-320,y,640,tag=="detail"and 100 or 52,false);return w
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
        self.paint_at=0;self.force_update=true
        if not self.failed then self.stage="waiting"end
    end -- world drop never touches old UObjects
    function self:set(stage,done,total,from_status)
        if self.failed then return end
        if not from_status and(self.ready or self.stage=="view")and preparation[stage]then return end
        local next_stage=labels[stage]and stage or "waiting"
        local previous_done,previous_total=self.done,self.total
        if self.stage~=next_stage then self.force_update=true end
        self.stage=next_stage;self.done,self.total=nil,nil;self.ready=false
        if stage=="assets"and type(done)=="number"and math.type(done)=="integer"and type(total)=="number"
            and math.type(total)=="integer"and total>0 and done>=0 and done<=total then
            self.done,self.total=done,total
            if previous_done~=done or previous_total~=total then self.force_update=true end
        end
    end
    function self:status(state,scene,own,reason)
        if state=="error"or state=="stopped"then self:fail(reason);return end
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
        elseif state=="wait_scene"and reason=="native scene assets loading"and self.stage=="present"then return
        elseif state=="wait_scene"and reason=="native gameplay pawn preparing"and preparation[self.stage]then return
        elseif state=="wait_scene"and reason=="native gameplay result is stale"then self:set("sync",nil,nil,true)
        elseif self.stage~="assets"then self:set("waiting")end
    end
    function self:fail(reason)
        if not self.failure_reason and type(reason)=="string"and reason~=""then
            local text=reason:gsub("%c"," ");local cut=math.min(#text,192)
            -- Keep a bounded UTF-8 prefix without cutting a multibyte character.
            while cut>0 and text:byte(cut+1)and text:byte(cut+1)>=0x80 and text:byte(cut+1)<=0xBF do cut=cut-1 end
            self.failure_reason=text:sub(1,cut)..(#text>cut and "..."or "")
        end
        self.failed=true;self.ready=false;self.stage="error";self.done,self.total=nil,nil;self.force_update=true
    end
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
            local now=env.now()
            if not self.force_update and now<self.paint_at then return true end
            local elapsed=math.max(0,math.floor(now-self.started_at))
            local assets_complete=self.total~=nil and self.done==self.total
            local title=(assets_complete and "Player assets loaded"or labels[self.stage])..(self.failed and ""or string.rep(".",math.floor(now*2)%4))
            local detail=self.failed and ((self.failure_reason and self.failure_reason.."\n"or "").."Close and join again to retry.\nElapsed: "..elapsed.."s")or self.total
                and string.format("Player assets: %d / %d  |  Elapsed: %ds",self.done,self.total,elapsed)
                    ..(assets_complete and "\nPlayer models and your view are still loading."or "")
                or "Elapsed: "..elapsed.."s"..(self.stage=="waiting"and "\nProgress is not available yet."or "")
            local signature=title..":"..detail
            if host.signature~=signature then
                checked(host.token,function()host.stage:SetText(env.text(title))end)
                checked(host.token,function()host.detail:SetText(env.text(detail))end)
                checked(host.token,function()host.bar:SetIsMarquee(self.total==nil or assets_complete)end)
                if self.total and not assets_complete then checked(host.token,function()host.bar:SetPercent(self.done/self.total)end)end
                host.signature=signature
            end
            self.paint_at=now+.5;self.force_update=false
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
