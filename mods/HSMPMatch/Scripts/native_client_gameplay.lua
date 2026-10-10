-- Native pawn bootstrap. Receipt, construction and possession are separate.
local M={}
local PENDING="native gameplay pawn preparing"
local preparation={begin="present",passport="character",construct="present",equipment="equipment",possess="controls",post_equipment="check_equipment"}
local BOOTSTRAP_KEYS={schema=true,actor_class=true,team=true,passport=true,construction=true,equipment=true}
local function fail(reason)error(reason,0)end
local function exact_integer(v)return math.type(v)=="integer" and v>=0 end
local function dense(v,count)
    if type(v)~="table"or getmetatable(v)~=nil or #v~=count then return false end
    for key in pairs(v)do if not exact_integer(key)or key==0 or key>count then return false end end
    for index=1,count do if rawget(v,index)==nil then return false end end
    return true
end
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
    local self={key=nil,token=nil,rows={},cursor=1,ready=false,timing_reports=0,timing_failure=false}
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
    local function passport_env(row,timing,read_clock,weapon_snapshot)
        local out={weapon_snapshot=weapon_snapshot};local null_class
        -- These copied, inclusive spans nest. They are not additive. The
        -- clock crosses only the scalar QPC endpoint; its overhead is not
        -- subtracted from either these spans or the original receipt age.
        local function observe(key,fn)
            if not timing then return fn end
            return function(...)
                local before=read_clock();if not before then return fn(...)end
                timing[key.."_n"]=(timing[key.."_n"]or 0)+1
                local result=table.pack(pcall(fn,...));local after=read_clock()
                if after and after>=before then timing[key.."_us"]=(timing[key.."_us"]or 0)+after-before
                else timing.timing_incomplete=true end
                if not result[1]then error(result[2],0)end
                return table.unpack(result,2,result.n)
            end
        end
        local pawn_current=observe("current",function()return current(row)end)
        out.guard=function()
            local ok,reason=guarded(require_api("native_gameplay_current"),row.handle,true)
            if ok~=true then fail(reason or"native gameplay original pawn guard unavailable")end
            return true
        end
        out.current=pawn_current
        out.resolve_class=function(path)
            local o=guarded(env.find,path)
            if not o then fail("native gameplay passport class unavailable")end
            local a=guarded(function()return o:GetAddress()end)
            local n=guarded(function()return o:GetFName()end)
            local full=guarded(function()return o:GetFullName()end)
            local kind,exact_path=full:match("^(%S+)%s+(.+)$")
            local again=guarded(env.find,path)
            if not again or guarded(function()return again:GetAddress()end)~=a or
                guarded(function()return again:GetFName()==n end)~=true or guarded(function()return again:type()end)~="UClass"or
                (kind~="Class"and kind~="BlueprintGeneratedClass")or exact_path~=path or
                guarded(function()return again:HasAnyFlags(0x40000000)end)~=false then fail("native gameplay passport class changed")end
            return again
        end
        out.null_class=function()
            if null_class then
                out.guard()
                if null_class:type()~="UClass"or null_class:GetAddress()~=0 then fail("explicit native null UClass changed")end
                out.guard();return null_class
            end
            if type(Passport.find_null_class)~="function"then fail("explicit native null UClass unavailable")end
            local o,why=Passport.find_null_class(out);if not o then fail(why or"explicit native null UClass unavailable")end;null_class=o;return o
        end
        out.fname=function(value)return guarded(env.fname,value)end
        local weapons={}
        out.weapon_guard=function(actor,field,id)
            local pawn=pawn_current();local original=guarded(function()return pawn[field]end)
            if not original then fail("native gameplay weapon field unavailable")end
            local function snapshot(o)
                local address=guarded(function()return o:GetAddress()end)
                if address==0 or guarded(function()return o:IsValid()end)~=true or
                    guarded(function()return o:HasAnyFlags(0x40000000)end)~=false then fail("native gameplay weapon expired")end
                local name=guarded(function()return o:GetFName()end)
                local class=guarded(function()return o:GetClass()end)
                local class_address=guarded(function()return class:GetAddress()end)
                local class_name=guarded(function()return class:GetFName()end)
                local world=guarded(function()return o:GetWorld()end)
                if not world or guarded(function()return world:GetAddress()end)~=env.world_address(self.token)then fail("native gameplay weapon world changed")end
                return{address=address,name=name,class_address=class_address,class_name=class_name}
            end
            local incoming=snapshot(actor);local fresh=snapshot(original)
            local old=weapons[id]
            for _,value in ipairs({incoming,old or fresh})do
                if value.address~=fresh.address or value.name~=fresh.name or value.class_address~=fresh.class_address or
                    value.class_name~=fresh.class_name then fail("native gameplay original weapon identity changed")end
            end
            -- Resolve the hard binding after all metadata/world callbacks.
            local latest=pawn_current();local item=guarded(function()return latest[field]end)
            if not item or guarded(function()return item:GetAddress()end)~=fresh.address or
                guarded(function()return item:GetFName()==fresh.name end)~=true then fail("native gameplay original weapon binding changed")end
            weapons[id]=old or fresh;return true
        end
        if timing then
            out.guard=observe("guard",out.guard)
            out.weapon_guard=observe("weapon_guard",out.weapon_guard)
            out.timing=timing;out.now_us=read_clock
        end
        return out
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
            local after=current(row)
            if match and guarded(function()return after:GetAddress()==pawn:GetAddress() and after:GetFName()==pawn:GetFName()end)then return true end
        end end
        return false
    end
    function self:view_ready(scene)
        return self.ready and type(scene)=="table" and scene.generation==self.key and scene.gameplay_proof==true
    end
    function self:sync(scene)
        if self.key and type(scene)=="table" and scene.generation~=self.key then self:clear()end
    end
    function self:apply(scene)
        local trace,started,phase_started,row_started,apply_returned,display_sampled_at
        local gear_clock_reads,gear_clock_limit,last_clock=0,32768,nil
        local function clock()
            if type(env.now_us)~="function"then return nil end
            local ok,value=pcall(env.now_us)
            if trace then trace.clock_reads=(trace.clock_reads or 0)+1 end
            if ok and math.type(value)=="integer"and value>=0 and(not last_clock or value>=last_clock)then last_clock=value;return value end
            trace=nil
        end
        local function phase(stage)
            if not trace then return end;local tick=clock();if not tick then trace=nil;return end
            trace[trace.stage.."_us"]=tick-phase_started;trace.stage=stage;phase_started=tick;return tick
        end
        local ok,result,reason=pcall(function()
            local key,own=identity(scene)
            local world,token,pc=env.world()
            if not world or not token or not pc or not env.same(token)then fail("native gameplay world unavailable")end
            if self.key and (self.key~=key or not env.same(self.token))then self:clear()end
            if not self.key then
                self.key=key;self.token=token;self.rows={};self.cursor=1;self.ready=false
                for _,entity in ipairs(scene.entities)do self.rows[#self.rows+1]={id=entity.id,incarnation=entity.incarnation,revision=entity.revision,recipe=entity.recipe,stage="begin"}end
            end
            local row=self.rows[self.cursor]
            if row then
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
                    if built~=true then fail(why or"native gameplay construction refused")end;row.stage="equipment"
                elseif row.stage=="equipment"then
                    if type(Passport.restore_live_armor)~="function"then fail("native gameplay live armor restoration unavailable")end
                    local restored,restore_reason=Passport.restore_live_armor(row.recipe,passport_env(row))
                    if restored~=true then fail(restore_reason or"native gameplay live armor restoration refused")end
                    if type(Passport.restore_live_weapons)~="function"then fail("native gameplay live weapon restoration unavailable")end
                    local equipped,equip_reason=Passport.restore_live_weapons(row.recipe,passport_env(row))
                    if equipped~=true then fail(equip_reason or"native gameplay live weapon restoration refused")end
                    local verified,why=Passport.verify_equipment(row.recipe,passport_env(row))
                    if verified~=true then fail(why or"native gameplay equipment differs from authority")end;row.stage="possess"
                elseif row.stage=="possess"then
                    local possessed,why=guarded(require_api("native_gameplay_finish"),row.handle)
                    if possessed~=true then fail(why or"native gameplay possession refused")end;row.stage="post_equipment"
                elseif row.stage=="post_equipment"then
                    local verified,why=Passport.verify_equipment(row.recipe,passport_env(row))
                    if verified~=true then fail(why or"native gameplay equipment changed after possession")end
                    row.stage="complete";self.cursor=self.cursor+1
                end
                return nil,PENDING
            end
            env.progress("sync")
            if scene.fresh~=true then return nil,"native gameplay result is stale"end
            if(scene.state==1 or scene.state==2)and type(env.diagnostic)=="function"and(self.timing_reports<8 or not self.timing_failure)then
                started=clock();if started then
                    phase_started=started;trace={epoch=scene.epoch,dir_seq=scene.dir_seq,frame_seq=scene.frame_seq,authority_tick=scene.authority_tick,
                        received_age_entry_ms=scene.received_age_ms,source_state=scene.state,stage="movement",rows={},clock_reads=1,
                        gear_clock_limit=gear_clock_limit,timing_inclusive=true,clock_overhead_subtracted=false}
                end
            end
            -- Replay only source-acknowledged movement/Run through the cooked
            -- pawn's actual input events, before authoritative reconciliation.
            for index,original in ipairs(self.rows)do
                local entity=scene.entities[index]
                if not exact_integer(entity.buttons)or(entity.buttons&~1)~=0 or type(entity.axes)~="table"or#entity.axes~=8 then fail("native gameplay first slice supports Move and Run only")end
                for axis,value in ipairs(entity.axes)do
                    if type(value)~="number"or value~=value or math.abs(value)==math.huge or math.abs(value)>1 or(axis>2 and value~=0)then fail("native gameplay movement result unsupported")end
                end
                local function event(name,...)
                    local pawn=current(original);local fn=guarded(function()return pawn[name]end)
                    local kind=type(fn)
                    if(kind~="userdata"and kind~="table")or guarded(function()return fn:type()end)~="UFunction"or
                        guarded(function()return fn:IsValid()end)~=true or
                        guarded(function()return fn:GetFullName()end)~="Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..name then
                        fail("native gameplay native movement event unavailable: "..name)
                    end
                    guarded(fn,pawn,...);current(original)
                end
                event("InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14",entity.axes[1])
                event("InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19",entity.axes[2])
                local run=(entity.buttons&1)~=0
                if original.run~=run then
                    event(run and"InpActEvt_Run_K2Node_InputActionEvent_4"or"InpActEvt_Run_K2Node_InputActionEvent_5",{KeyName=guarded(env.fname,"None")})
                    original.run=run
                end
            end
            phase("native_apply")
            local applied,actual=guarded(require_api("native_gameplay_apply"),scene)
            if applied~=true then return nil,actual end
            if type(actual)~="table" or actual.generation~=self.key or actual.gameplay_proof~=true then fail("native gameplay complete native proof unavailable")end
            apply_returned=phase("weapon_batch")
            if trace then trace.applied_received_age_ms=actual.received_age_ms;trace.applied_authority_tick=actual.authority_tick;trace.applied_frame_seq=actual.frame_seq end
            local weapon_batch,batch_reason=guarded(require_api("native_gameplay_weapons"),actual)
            if not weapon_batch then fail(batch_reason or"native gameplay complete weapon readback unavailable")end
            if type(weapon_batch)~="table"or getmetatable(weapon_batch)~=nil or weapon_batch.epoch~=actual.epoch or
                weapon_batch.dir_seq~=actual.dir_seq or weapon_batch.authority_tick~=actual.authority_tick or
                not dense(weapon_batch.entities,#self.rows)then fail("native gameplay weapon result changed")end
            for index,original in ipairs(self.rows)do
                local copied=weapon_batch.entities[index]
                if type(copied)~="table"or getmetatable(copied)~=nil or copied.id~=original.id or
                    copied.incarnation~=original.incarnation then fail("native gameplay weapon pawn changed")end
            end
            phase("gear")
            for index,original in ipairs(self.rows)do
                if trace then row_started=clock();if not row_started then trace=nil else
                    trace.active_row=index;trace.rows[index]={id=original.id,incarnation=original.incarnation,us=0}
                end end
                local detail=trace and trace.rows[index]
                local function gear_clock()
                    if not trace or detail.timing_incomplete then return nil end
                    if gear_clock_reads>=gear_clock_limit then detail.timing_incomplete=true;return nil end
                    gear_clock_reads=gear_clock_reads+1;detail.clock_reads=(detail.clock_reads or 0)+1
                    return clock()
                end
                local verified,why=Passport.verify_equipment(original.recipe,passport_env(original,detail,gear_clock,weapon_batch.entities[index]))
                if verified~=true then fail(why or"native gameplay complete equipment readback failed")end
                if trace then local tick=clock();if not tick then trace=nil else trace.rows[index].us=tick-row_started;trace.active_row=nil end end
            end
            local confirming=phase("confirm")
            if trace and confirming and apply_returned then trace.elapsed_apply_return_to_confirm_us=confirming-apply_returned end
            -- Age in the returned native result was sampled during this call.
            -- Keep an earlier same-clock anchor for passive display expiry.
            local sampled_us=clock()
            if sampled_us then display_sampled_at=sampled_us/1000000 end
            local confirmed,final=guarded(require_api("native_gameplay_confirm"),actual)
            if confirmed~=true then return nil,final end
            if type(final)~="table"or final.generation~=self.key or final.gameplay_proof~=true then fail("native gameplay final native proof unavailable")end
            self.ready=true;return true,final
        end)
        if not(ok and result==true)then self.ready=false end
        if trace then
            local tick=clock();if tick then
                trace[trace.stage.."_us"]=tick-phase_started;trace.total_us=tick-started
                if trace.active_row then trace.rows[trace.active_row].us=tick-row_started;trace.active_row=nil end
                trace.ok=ok and result==true;trace.reason=(not ok and tostring(result)or type(reason)=="string"and reason or""):sub(1,256)
                trace.gear_clock_reads=gear_clock_reads
                if self.timing_reports<8 or(not trace.ok and not self.timing_failure)then
                    if self.timing_reports<8 then self.timing_reports=self.timing_reports+1
                    elseif not trace.ok then self.timing_failure=true end
                    pcall(env.diagnostic,trace)
                end
            end
        end
        if not ok then self.ready=false;return nil,tostring(result)end
        return result,reason,result==true and display_sampled_at or nil
    end
    return self
end
M.PENDING=PENDING
-- First native gameplay acceptance exercises Move+Run only. Combat action RPCs
-- require their own native authoritative result and readback proof.
function M.intent(scene,own)
    if type(scene)~="table"or scene.gameplay_proof~=true or scene.fresh~=true or type(own)~="table"then return nil,"native applied scene is stale"end
    return{1,0,0,0,0,0,0,0},1
end
return M
