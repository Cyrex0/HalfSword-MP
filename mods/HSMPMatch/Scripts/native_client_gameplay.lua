-- Native pawn bootstrap, then display-only puppets. Each original pawn is built
-- once with its authority recipe (passport, armour, weapons, possession); after
-- that every frame shows the authority's played-back physical pose. Combat
-- physics runs only on the authority.
local M={}
local PENDING="native gameplay pawn preparing"
local LOST="native gameplay puppet lost"
-- Blueprint polling delays can keep native latent work pending forever; past
-- this wait the stage advances (native finish applies the same bound).
local SETTLE_US=3000000
local MAX_REBUILDS=3
local GOOD_FRAMES_RESET=600
local STALL_FRAMES=120
local preparation={begin="present",passport="character",construct="present",native_setup="native_setup",equipment="equipment",post_setup="native_setup",possess="controls",post_equipment="check_equipment"}
local BOOTSTRAP_KEYS={schema=true,actor_class=true,team=true,passport=true,construction=true,equipment=true}
local function fail(reason)error(reason,0)end
local function exact_integer(v)return math.type(v)=="integer" and v>=0 end
local function bootstrap(recipe)
    if type(recipe)~="table" or getmetatable(recipe)~=nil or recipe.schema~=1 then fail("native gameplay bootstrap schema unavailable")end
    for key in pairs(recipe)do if not BOOTSTRAP_KEYS[key]then fail("native gameplay bootstrap field unsupported")end end
    for key in pairs(BOOTSTRAP_KEYS)do if rawget(recipe,key)==nil then fail("native gameplay complete bootstrap unavailable")end end
    if type(recipe.actor_class)~="string" or recipe.actor_class=="" or math.type(recipe.team)~="integer" or
        type(recipe.passport)~="table" or type(recipe.construction)~="table" or type(recipe.equipment)~="table"then
        fail("native gameplay bootstrap fields unavailable")
    end
end
local function identity(scene)
    if type(scene)~="table" or type(scene.generation)~="string" or scene.generation=="" or
        math.type(scene.epoch)~="integer" or scene.epoch==0 or not exact_integer(scene.dir_seq)then fail("native gameplay scene generation unavailable")end
    local own,seen=nil,{}
    if type(scene.entities)~="table" or #scene.entities==0 or #scene.entities>32 then fail("native gameplay complete roster unavailable")end
    for index,row in ipairs(scene.entities)do
        if type(row)~="table" or row.epoch~=scene.epoch or not exact_integer(row.id) or row.id==0 or
            not exact_integer(row.incarnation) or not exact_integer(row.revision) or row.revision==0 or
            type(row.recipe)~="table" or seen[row.id]then fail("native gameplay recipe binding unavailable")end
        bootstrap(row.recipe)
        seen[row.id]=index
        if row.owner_peer==scene.peer_id and row.kind==0 then
            if own then fail("native gameplay ambiguous owned human")end;own=row
        end
    end
    if not own then fail("native gameplay owned human unavailable")end
    return scene.generation,own
end
function M.new(env)
    local N,Passport=env.native,env.passport
    local self={key=nil,token=nil,rows={},cursor=1,ready=false,rebuilds=0,frame_reports=0,gear_warnings={}}
    local function require_api(name)
        if type(N[name])~="function"then fail("native gameplay API unavailable: "..name)end;return N[name]
    end
    local function guarded(fn,...)
        if not self.token or not env.same(self.token)then fail("native gameplay world changed")end
        local result=table.pack(fn(...))
        if not env.same(self.token)then fail("native gameplay world changed")end
        return table.unpack(result,1,result.n)
    end
    local function current(row)
        local pawn,reason=guarded(require_api("native_gameplay_current"),row.handle)
        if not pawn then fail(reason or"native gameplay original pawn unavailable")end
        return pawn
    end
    local function now_us()
        if type(env.now_us)~="function"then return nil end
        local ok,value=pcall(env.now_us)
        if ok and math.type(value)=="integer"and value>=0 then return value end
    end
    local function settled(row,pending)
        if pending==0 then row.wait_since=nil;return true end
        local now=now_us();if not now then return false end
        row.wait_since=row.wait_since or now
        return now-row.wait_since>=SETTLE_US
    end
    local function passport_env(row)
        local out={};local null_class
        out.guard=function()
            local ok,reason=guarded(require_api("native_gameplay_current"),row.handle,true)
            if ok~=true then fail(reason or"native gameplay original pawn guard unavailable")end
            return true
        end
        out.current=function()return current(row)end
        out.resolve_class=function(path)
            local o=guarded(env.find,path)
            if not o then fail("native gameplay passport class unavailable")end
            local a=guarded(function()return o:GetAddress()end)
            local full=guarded(function()return o:GetFullName()end)
            local kind,exact_path=full:match("^(%S+)%s+(.+)$")
            if guarded(function()return o:type()end)~="UClass"or(kind~="Class"and kind~="BlueprintGeneratedClass")or exact_path~=path or
                a==0 or guarded(function()return o:HasAnyFlags(0x40000000)end)~=false then fail("native gameplay passport class changed")end
            return o
        end
        out.null_class=function()
            if null_class then
                if null_class:type()~="UClass"or null_class:GetAddress()~=0 then fail("explicit native null UClass changed")end
                return null_class
            end
            if type(Passport.find_null_class)~="function"then fail("explicit native null UClass unavailable")end
            local o,why=Passport.find_null_class(out);if not o then fail(why or"explicit native null UClass unavailable")end;null_class=o;return o
        end
        out.fname=function(value)return guarded(env.fname,value)end
        out.weapon_guard=function(actor,field)
            local pawn=current(row);local item=guarded(function()return pawn[field]end)
            if not item or guarded(function()return item:GetAddress()end)~=guarded(function()return actor:GetAddress()end)or
                guarded(function()return item:IsValid()end)~=true or guarded(function()return item:HasAnyFlags(0x40000000)end)~=false then
                fail("native gameplay original weapon binding changed")
            end
            return true
        end
        return out
    end
    -- Gear is checked once against the authority recipe. A mismatch is cosmetic
    -- on a display-only puppet: it is reported, never fatal.
    local function check_gear(row,stage)
        local ok,why=Passport.verify_equipment(row.recipe,passport_env(row))
        if ok~=true then
            self.gear_warnings[#self.gear_warnings+1]={id=row.id,stage=stage,reason=tostring(why or"equipment differs from authority")}
            if type(env.diagnostic)=="function"then pcall(env.diagnostic,{kind="gear",id=row.id,stage=stage,reason=tostring(why or"")})end
        end
    end
    function self:drop()
        require_api("native_gameplay_clear")(true)
        self.key=nil;self.token=nil;self.rows={};self.cursor=1;self.ready=false
    end
    function self:clear()
        local forgot=self.token and not env.same(self.token) or false
        local ok,reason=require_api("native_gameplay_clear")(forgot)
        self.key=nil;self.token=nil;self.rows={};self.cursor=1;self.ready=false
        if ok~=true then fail(reason or"native gameplay cleanup refused")end
    end
    function self:owned(pawn)
        if not self.token or not env.same(self.token)then return false end
        for _,row in ipairs(self.rows)do if row.handle then
            local original=current(row)
            local match=guarded(function()return original:GetAddress()==pawn:GetAddress() and original:GetFName()==pawn:GetFName() and
                original:GetClass():GetAddress()==pawn:GetClass():GetAddress() and pawn:HasAnyFlags(0x40000000)==false end)
            if match then return true end
        end end
        return false
    end
    -- Every original pawn is built; the controller may request recipe-free scenes.
    function self:bootstrapped()
        return self.key~=nil and #self.rows>0 and self.cursor>#self.rows
    end
    function self:view_ready(scene)
        return self.ready and type(scene)=="table" and scene.generation==self.key and scene.gameplay_proof==true
    end
    function self:sync(scene)
        if self.key and type(scene)=="table" and scene.generation~=self.key then self:clear()end
    end
    local function build(scene)
        local key=identity(scene)
        local world,token,pc=env.world()
        if not world or not token or not pc or not env.same(token)then fail("native gameplay world unavailable")end
        if self.key and (self.key~=key or not env.same(self.token))then self:clear()end
        if not self.key then
            self.key=key;self.token=token;self.rows={};self.cursor=1;self.ready=false
            for _,entity in ipairs(scene.entities)do self.rows[#self.rows+1]={id=entity.id,incarnation=entity.incarnation,revision=entity.revision,recipe=entity.recipe,stage="begin"}end
        end
        local row=self.rows[self.cursor]
        local entity=scene.entities[self.cursor]
        if not entity or entity.id~=row.id or entity.incarnation~=row.incarnation or entity.revision~=row.revision then fail("client gameplay generation")end
        env.progress(preparation[row.stage])
        if row.stage=="begin"then
            local handle,pawn=guarded(require_api("native_gameplay_begin"),scene,world:GetAddress(),pc:GetAddress(),entity.id)
            if not handle then fail(pawn or"native gameplay deferred spawn refused")end
            row.handle=handle;row.stage="passport"
        elseif row.stage=="passport"then
            local applied,why=Passport.before_finish(row.recipe,passport_env(row))
            if applied~=true then fail(why or"native gameplay passport refused")end;row.stage="construct"
        elseif row.stage=="construct"then
            local built,why=guarded(require_api("native_gameplay_construct"),row.handle)
            if built~=true then fail(why or"native gameplay construction refused")end;row.stage="native_setup"
        elseif row.stage=="native_setup"or row.stage=="post_setup"then
            local pending,why=guarded(require_api("native_gameplay_initialized"),row.handle)
            if pending==nil then fail(why or"native gameplay initialization unavailable")end
            if not exact_integer(pending)then fail("native gameplay pending action count unavailable")end
            if settled(row,pending)then
                row.wait_since=nil
                if row.stage=="post_setup"then check_gear(row,"post_setup");row.stage="possess"
                else row.stage="equipment"end
            end
        elseif row.stage=="equipment"then
            if type(Passport.restore_live_armor)~="function"then fail("native gameplay live armor restoration unavailable")end
            local restored,restore_reason=Passport.restore_live_armor(row.recipe,passport_env(row))
            if restored~=true then fail(restore_reason or"native gameplay live armor restoration refused")end
            if type(Passport.restore_live_weapons)~="function"then fail("native gameplay live weapon restoration unavailable")end
            local equipped,equip_reason=Passport.restore_live_weapons(row.recipe,passport_env(row))
            if equipped~=true then fail(equip_reason or"native gameplay live weapon restoration refused")end
            row.stage="post_setup"
        elseif row.stage=="possess"then
            local possessed,why=guarded(require_api("native_gameplay_finish"),row.handle)
            if possessed==false and why=="native gameplay initialization remains pending"then row.stage="post_setup";return nil,PENDING end
            if possessed~=true then fail(why or"native gameplay possession refused")end;row.stage="post_equipment"
        elseif row.stage=="post_equipment"then
            check_gear(row,"post_equipment")
            row.stage="complete";self.cursor=self.cursor+1
        end
        return nil,PENDING
    end
    function self:apply(scene)
        local ok,result,reason=pcall(function()
            if self:bootstrapped()and type(scene)=="table"and scene.generation~=self.key then
                -- A new roster arrives as a recipe-free view; rebuild from the next full scene.
                self:clear();return nil,PENDING
            end
            if not self:bootstrapped()then return build(scene)end
            if not env.same(self.token)then fail("native gameplay world changed")end
            env.progress("sync")
            local started=now_us()
            local applied,actual=guarded(require_api("native_gameplay_puppet"),scene)
            if applied~=true then
                if type(actual)=="string"and actual:sub(1,#LOST)==LOST then
                    self.rebuilds=self.rebuilds+1
                    self.good_frames=0
                    if self.rebuilds>MAX_REBUILDS then fail(actual)end
                    if type(env.diagnostic)=="function"then pcall(env.diagnostic,{kind="rebuild",reason=actual,rebuilds=self.rebuilds})end
                    self:clear();return nil,PENDING
                end
                fail(actual or"native gameplay puppet refused")
            end
            if type(actual)~="table"or actual.generation~=self.key or actual.gameplay_proof~=true then fail("native gameplay puppet result unavailable")end
            local finished=now_us()
            if started and finished and type(env.diagnostic)=="function"and self.frame_reports<8 then
                self.frame_reports=self.frame_reports+1
                pcall(env.diagnostic,{kind="frame",us=finished-started,received_age_ms=actual.received_age_ms,authority_tick=actual.authority_tick,
                    fresh=actual.fresh,puppet_frames=actual.puppet_frames})
            end
            self.good_frames=(self.good_frames or 0)+1
            -- Losses far apart are independent; only a burst should stop the client.
            if self.good_frames>=GOOD_FRAMES_RESET then self.rebuilds=0 end
            if self.good_frames==STALL_FRAMES and actual.puppet_frames==0 and type(env.diagnostic)=="function"then
                pcall(env.diagnostic,{kind="stall",reason="no authority pose published",frames=self.good_frames})
            end
            self.ready=true
            return true,actual
        end)
        if not ok then self.ready=false;return nil,tostring(result)end
        if result~=true then self.ready=false end
        return result,reason,result==true and (now_us() or 0)/1000000 or nil
    end
    return self
end
M.PENDING=PENDING
M.LOST=LOST
return M
