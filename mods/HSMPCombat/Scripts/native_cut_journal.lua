-- Bounded developer-only POST evidence. No marker baseline, accepted wear
-- history or relay authority is inferred from a Blueprint callback.
local M={MAX_ATTEMPTS=3,MAX_EVENTS=16,MAX_RESPONSES=2,MARKER_LIMIT=32,TAG_LIMIT=16,SECONDS=15}
M.INIT={
    {"Component 1 (Weapon)","ref"},{"Bone Name 1","name"},{"Component 2 (Body)","ref"},{"Bone Name 2","name"},
    {"Tip","ref"},{"Thrust?","bool"},{"My Weapon","ref"},{"Hit Actor","ref"},{"Start Weapon Impact","vec"},
    {"Base","ref"},{"Initial Impact Point","vec"},{"Edge Alignment","number"},{"Edge Direction","vec"},
    {"Initiail Norm","vec"},{"Dismember Cut Level","integer"},{"Edge Sharpness","number"},{"Tip Sharpness","number"},
    {"Material Density","number"},{"Hit Box Collision","ref"},{"BoxCollision","bool"},{"Weapon Hit Module","ref"},
    {"False Egde","bool"},{"Box Extent","vec"},{"Spikes (temp)","bool"},{"Draw Cut","number"}}
M.HOOKS={
    {"constraint_begin","/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:ReceiveBeginPlay"},
    {"wear","/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:Dismemberment Check"},
    {"cut_attempt","/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:Call Dismember"},
    {"initiate","Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Initiate"},
    {"delayed","Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Delayed"}}
local function finite(v)return type(v)=="number"and v==v and math.abs(v)<math.huge end
local function integer(v)return finite(v)and math.tointeger(v)end
local function text(v,limit)return type(v)=="string"and #v>0 and #v<=limit and not v:find("\0",1,true)and v or nil end
local function unknown(reason)return {available=false,reason=reason}end
local function copy(v,depth,count)
    count=count or {n=0};count.n=count.n+1;assert(count.n<=4096,"scalar copy capacity")
    if type(v)=="table"then
        assert((depth or 0)<12,"scalar copy depth");local out,n={},0
        for k,x in pairs(v)do n=n+1;assert(n<=128 and (type(k)=="string"or integer(k)),"scalar copy keys");out[k]=copy(x,(depth or 0)+1,count)end
        return out
    end
    assert(v==nil or type(v)=="boolean"or finite(v)or type(v)=="string"and #v<=2048,"non-scalar evidence")
    return v
end
function M.new(o)
    local s={armed=false,used=false,attempts=0,events=0,responses=0,generation=0,installed={},uncertain={},initializers={}}
    local function now()local ok,v=pcall(o.now);return ok and finite(v)and v or nil end
    function s:stop(reason)self.generation=self.generation+1;self.armed=false;if self.started then self.used=true end;self.reason=reason or "stopped"end
    function s:start()
        if o.enabled()~=true or self.used then return false end
        local t=now();if not t then return false end
        if not self.armed then self.generation=self.generation+1;self.started=true;self.armed=true;self.deadline=t+M.SECONDS end
        return true
    end
    function s:pending()
        if not self.armed or o.enabled()~=true then return false end
        local t=now()
        if not t or t>self.deadline then self:stop("time budget");self.used=true;return false end
        if self.events>=M.MAX_EVENTS or self.attempts>=M.MAX_ATTEMPTS then self:stop("capture budget");self.used=true;return false end
        return true
    end
    function s:install(register,callback)
        if not self:pending()then return false end
        local all=true
        for _,h in ipairs(M.HOOKS)do
            local kind,path=h[1],h[2]
            if not self.installed[kind]and not self.uncertain[kind]then
                local ok,a,b=pcall(register,path,function(...)callback(kind,...)end)
                local good=ok and integer(a)and integer(b)and a>=-2147483648 and a<=2147483647 and a==b
                self.installed[kind]=good==true or type(good)=="number"
                self.uncertain[kind]=ok and not self.installed[kind]
            end
            if not self.installed[kind]then all=false end
        end
        return all
    end
    function s:response(value)
        if not self:pending()or self.responses>=M.MAX_RESPONSES then return nil end
        self.responses=self.responses+1
        local token=self.generation
        local ok,row=pcall(function()return {schema=1,kind="owner_damage_response",phase="approved_owner_call_returned",
            authority=false,relay_eligible=false,observed_s=now(),response=copy(value)}end)
        if not ok or token~=self.generation or not self.armed then return nil end
        pcall(o.emit,row);return row
    end
    function s:capture(kind,resolve)
        if not self:pending()then return nil end -- before unwrapping parameters/native reads
        self.events=self.events+1
        local token,started=self.generation,now()
        local lost=false
        local function admitted()
            if lost then return false end
            local t=now()
            if self.generation~=token or not self.armed or o.enabled()~=true or not t or t>self.deadline then lost=true;return false end
            return true
        end
        local ok,e=pcall(resolve,admitted)
        if not ok or type(e)~="table"or type(e.current)~="function"or type(e.context)~="table"then
            self.attempts=self.attempts+1;return nil,"context unavailable"
        end
        local function current()
            if not admitted()then return false end
            local yes,value=pcall(e.current)
            if not yes or value~=true then lost=true;return false end
            return true
        end
        local function read(fn)
            assert(current(),"scope changed");local value=fn();assert(current(),"scope changed");return value
        end
        local function observed(fn)
            local yes,value=pcall(fn)
            if lost then error("scope changed",0)end
            return yes and {available=true,value=value}or unknown("native read unavailable")
        end
        local function name(v)
            if type(v)~="string"then v=read(function()return v:ToString()end)end
            return assert(text(v,256),"name unavailable")
        end
        local function ref(v)
            assert(v and read(function()return v:IsValid()end)==true,"object unavailable")
            local a=read(function()return v:GetAddress()end);assert(integer(a)and a>0,"object address unavailable")
            local f=read(function()return v:GetFName()end)
            return {address=a,name=name(f)}
        end
        local function vector(v,keys)
            local out={};for _,k in ipairs(keys or {"X","Y","Z"})do
                local x=read(function()return v[k]end);assert(finite(x),"vector unavailable");out[k]=x
            end;return out
        end
        local function typed(v,ty)
            if ty=="ref"then return ref(v)elseif ty=="name"then return name(v)elseif ty=="vec"then return vector(v)end
            if ty=="bool"then assert(type(v)=="boolean","boolean unavailable")
            elseif ty=="integer"then assert(integer(v),"integer unavailable")else assert(finite(v),"number unavailable")end
            return v
        end
        local function tags(v)
            local n=read(function()return v:GetArrayNum()end);assert(integer(n)and n>=0 and n<=M.TAG_LIMIT,"tag count unavailable")
            local values={};read(function()v:ForEach(function(_,p)
                assert(current(),"scope changed");assert(#values<n,"tag iteration capacity")
                values[#values+1]=name(read(function()return o.unwrap(p)end))
            end)end)
            assert(read(function()return v:GetArrayNum()end)==n and #values==n,"tag count changed")
            return values -- exact strings/order, not normalized tokens or presumed HP100
        end
        local function geometry(v,box)
            local r={identity=ref(v)}
            r.owner=observed(function()return ref(read(function()return v:GetOwner()end))end)
            r.attachment=observed(function()return ref(read(function()return v:GetAttachParent()end))end)
            r.attachment_bone=observed(function()return name(read(function()return v:GetAttachSocketName()end))end)
            r.tags=observed(function()return tags(read(function()return v.ComponentTags end))end)
            r.transform=observed(function()
                local t=read(function()return v:K2_GetComponentToWorld()end)
                return {translation=vector(read(function()return t.Translation end)),scale=vector(read(function()return t.Scale3D end)),
                    rotation=vector(read(function()return t.Rotation end),{"X","Y","Z","W"})}
            end)
            if box then
                r.extent=observed(function()
                    local q=vector(read(function()return v:GetUnscaledBoxExtent()end))
                    for _,k in ipairs({"X","Y","Z"})do assert(q[k]>=0 and q[k]<=10000,"cut extent unavailable")end
                    return q -- separate cut-box contract; native Y500 is valid, not a DCD bound
                end)
            end
            return r
        end
        local function markers(a)
            local n=read(function()return a:GetArrayNum()end);assert(integer(n)and n>=0 and n<=M.MARKER_LIMIT,"marker count unavailable")
            local values={};read(function()a:ForEach(function(_,p)
                assert(current(),"scope changed");assert(#values<n,"marker iteration capacity")
                values[#values+1]=geometry(read(function()return o.unwrap(p)end),false)
            end)end)
            assert(read(function()return a:GetArrayNum()end)==n and #values==n,"marker count changed")
            local ids={};read(function()a:ForEach(function(_,p)
                assert(current(),"scope changed");assert(#ids<n,"marker recheck capacity")
                ids[#ids+1]=ref(read(function()return o.unwrap(p)end))
            end)end)
            assert(read(function()return a:GetArrayNum()end)==n and #ids==n,"marker recheck incomplete")
            for i,id in ipairs(ids)do assert(id.address==values[i].identity.address and id.name==values[i].identity.name,"marker membership changed")end
            return {count=n,values=values}
        end
        local initializer_key,initializer_value
        local result_ok,r=pcall(function()
            assert(current(),"scope changed")
            local row={schema=1,kind=kind,phase="Blueprint_POST",pre_available=false,baseline_available=false,history_complete=false,
                relay_eligible=false,authority=false,context=copy(e.context),observed_s=now(),parent=copy(e.parent),
                history_reason="shared marker baseline and other/earlier contributors unavailable; POST is not PRE"}
            if e.constraint then
                row.constraint=ref(e.constraint)
                local key=assert(text(e.context.scope_key,2048),"scope key unavailable")..":"..tostring(row.constraint.address)..":"..row.constraint.name
                if not self.initializers[key]then
                    assert(self.initializer_count==nil or self.initializer_count<4,"constraint capacity")
                    local init={fields={},complete=true,observation="first observed POST, not constructor PRE"}
                    for _,d in ipairs(M.INIT)do
                        init.fields[d[1]]=observed(function()return typed(read(function()return e.constraint[d[1]]end),d[2])end)
                        if not init.fields[d[1]].available then init.complete=false end
                    end
                    initializer_key,initializer_value=key,copy(init);row.initializer=init
                else row.initializer_available=true;row.initializer_observation="earlier copied POST"end
                row.damage=observed(function()local v=read(e.damage);assert(finite(v),"Damage unavailable");return v end)
                row.native={}
                for _,d in ipairs({{"Gore Rate","number"},{"Is Ammo","bool"},{"Thrust?","bool"},{"Draw Cut","number"},
                    {"Dismember Cut Level","integer"},{"Enum_DismembermentPart","integer"},{"Dismemberment In Progress","bool"}})do
                    row.native[d[1]]=observed(function()return typed(read(function()return e.constraint[d[1]]end),d[2])end)
                end
                row.markers=observed(function()return markers(read(function()return e.constraint["Overlapped Markers"]end))end)
                row.current_markers=observed(function()return markers(read(function()return e.constraint["Overlapped Markers Current"]end))end)
            else
                row.seven={}
                for i,k in ipairs({"master","part","attach_marker","markers","box1","box2","weapon"})do
                    row.seven[k]=observed(function()
                        local v=read(function()return o.unwrap(e.args[i])end)
                        if i==2 then assert(integer(v)and v>=0 and v<=14,"part unavailable");return v end
                        if i==4 then return markers(v)end
                        return geometry(v,i==5 or i==6)
                    end)
                end
                row.selected_part=observed(function()return typed(read(function()return e.victim["Currently Dismembered Part"]end),"integer")end)
                row.selected_master=observed(function()return ref(read(function()return e.victim["Currently Dismembered Mesh"]end))end)
                row.process=observed(function()return typed(read(function()return e.victim["Dismemberment In Process"]end),"bool")end)
                row.topology=observed(function()return assert(read(function()return e.topology()end),"topology unavailable")end)
            end
            assert(current(),"scope changed")
            if e.finish then assert(read(e.finish)==true,"final binding changed")end
            local ended=now();row.capture_elapsed_ms=started and ended and (ended-started)*1000 or nil
            return copy(row)
        end)
        if not result_ok then self.attempts=self.attempts+1;return nil,lost and "scope changed"or "read unavailable"end
        if initializer_key then self.initializers[initializer_key]=initializer_value;self.initializer_count=(self.initializer_count or 0)+1 end
        pcall(o.emit,r);return r
    end
    return s
end
return M
