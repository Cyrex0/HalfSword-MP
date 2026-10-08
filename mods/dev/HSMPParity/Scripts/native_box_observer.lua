-- Bounded read-only control. Native entry/post pairs are never restoration or damage authority.
local M={}
local objects={"world","pawn","mesh","box","box_owner"}
local fields={"match_id","round","life","world_key","owner_life","owner_pawn","owner_mesh","peer","side","grip"}
local function positive(v)return type(v)=="number" and v>0 and v<math.huge and v%1==0 end
local function hook_id(v)return type(v)=="number" and v%1==0 and v>=-2147483648 and v<=2147483647 end
local function process_counter(s,key)
    local v=type(s)=="table" and s[key]
    return type(v)=="number" and v%1==0 and v>=0 and v<=4294967295 and tostring(v) or "unavailable"
end
local admissions={allowed=true,would_block=true,mutex_poisoned=true,native_poisoned=true,
    frame_thread_unset=true,wrong_thread=true,panic=true}
local function admission(s)
    local v=type(s)=="table" and s.last_refused_caller_admission
    return type(v)=="string" and s.last_refused_caller_admission_available==true and admissions[v] and v or "unavailable"
end
local function status_reason(s)
    local v=type(s)=="table" and s.reason
    if type(v)~="string"then return "unavailable"end
    return v:sub(1,128):gsub("[%c\\]",function(c)return string.format("\\x%02x",c:byte())end)
end
local function copy(s)
    if type(s)~="table" then return nil end
    local c={}
    for _,k in ipairs(fields)do
        local v=s[k]
        if type(v)~="number" and type(v)~="string" then return nil end
        c[k]=v
    end
    if not positive(c.match_id) or not positive(c.round) or not positive(c.life) or not positive(c.owner_life)
        or not positive(c.owner_pawn) or not positive(c.owner_mesh) or not positive(c.peer) or not positive(c.grip)
        or c.world_key=="" or (c.side~="r" and c.side~="l") then return nil end
    for _,k in ipairs(objects)do
        local o=s[k]
        if type(o)~="table" or not positive(o.address) or type(o.path)~="string" or o.path:sub(1,1)~="/"
            or #o.path>2048 or o.path:find("%z") then return nil end
        c[k]={address=o.address,path=o.path}
    end
    return c -- Plain scalar copies; no native wrapper retained.
end
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(fields)do if a[k]~=b[k] then return false end end
    for _,k in ipairs(objects)do if a[k].address~=b[k].address or a[k].path~=b[k].path then return false end end
    return true
end
function M.new(api)
    local active,pending,hooks=nil,nil,{}
    local control_epoch=0
    local timing_count,timing_id=0,0
    local function clock()
        local ok,v=pcall(api.clock_ms or function()return os.clock()*1000 end)
        return ok and type(v)=="number" and v==v and math.abs(v)<math.huge and v or nil
    end
    local function timing(t,stage,status,play)
        if not t or timing_count>=32 then return end
        timing_count=timing_count+1
        pcall(function() -- Optional timing formatting/logging cannot change observer control.
        local fields={}
        local function field(key,value)
            local available=type(value)=="number" and value==value and math.abs(value)<math.huge
            fields[#fields+1]=key.."="..(available and tostring(value) or "unknown").." "..key.."_available="..tostring(available)
        end
        for _,key in ipairs({"command_enter_ms","snapshot_enter_ms","snapshot_exit_ms","begin_enter_ms","begin_exit_ms",
            "install_enter_ms","install_exit_ms","fresh_enter_ms","fresh_exit_ms","activate_enter_ms","activate_exit_ms"})do field(key,t[key])end
        for _,p in ipairs({{"snapshot_ms","snapshot_enter_ms","snapshot_exit_ms"},{"begin_ms","begin_enter_ms","begin_exit_ms"},
            {"install_ms","install_enter_ms","install_exit_ms"},{"fresh_ms","fresh_enter_ms","fresh_exit_ms"},
            {"activate_ms","activate_enter_ms","activate_exit_ms"}})do
            local a,b=t[p[2]],t[p[3]];field(p[1],a and b and b>=a and b-a or nil)
        end
        for _,key in ipairs({"local_ms","sample_now_ms","age_ms","generation"})do
            field("playback_"..key,type(play)=="table" and rawget(play,key) or nil)
        end
        api.log("BOXOBS_TIMING id=%d stage=%s status=%s record=%d %s authority=false",t.id,stage,status,timing_count,table.concat(fields," "))
        end)
    end
    local function native()
        local n=api.native()
        return type(n)=="table" and type(n.begin)=="function" and type(n.prepare)=="function" and type(n.activate)=="function" and type(n.mark)=="function"
            and type(n.stop)=="function" and type(n.read)=="function" and type(n.status)=="function" and n or nil
    end
    local function drain(n)
        local ok,rows=pcall(n.read)
        if not ok or type(rows)~="table" then api.log("BOXOBS unavailable readback");return end
        if #rows>32 then api.log("BOXOBS refused oversize native readback");return end
        for _,row in ipairs(rows)do
            -- The native budget is per activation, including nested functions.
            if active and active.drained>=32 then return end
            if active then active.drained=active.drained+1 end
            api.emit(row)
        end
    end
    local stopping=false
    local function stop(reason)
        control_epoch=control_epoch+1
        if stopping then return end
        stopping=true
        local admitted,n=pcall(native)
        if not admitted then n=nil end
        local status_fields
        if n then
            if active or pending then
                -- Status uses copied native scalars only; no snapshot resolver or UE object reads.
                local ok,s=pcall(n.status)
                if not ok then s=nil end
                local copied,values=pcall(function()return {status_reason(s),process_counter(s,"proven_thread_id"),process_counter(s,"foreign_callbacks_process_total"),
                    process_counter(s,"same_thread_unavailable_process_total"),process_counter(s,"unknown_thread_callbacks_process_total"),admission(s)}end)
                if copied then status_fields=values end
            end
            pcall(n.stop);pcall(drain,n)
        end
        active,pending=nil,nil
        if status_fields then
            pcall(api.log,"BOXOBS_STATUS stage=stop native_reason=%s proven_thread_id=%s foreign_callbacks_process_total=%s same_thread_unavailable_process_total=%s unknown_thread_callbacks_process_total=%s last_refused_caller_admission=%s foreign_callback_targets_known=false authority=false",table.unpack(status_fields))
        end
        pcall(api.log,"BOXOBS stopped reason=%s authority=false",reason or "developer")
        stopping=false
    end
    local function fresh(caller)
        if not active then return nil,"inactive" end
        local t=active.timing
        local first=t and not t.fresh_seen
        if first then t.fresh_seen=true;t.fresh_enter_ms=clock()end
        local ok,s,reason,play,eligible=pcall(api.snapshot,active.scope.peer,active.scope.side)
        if first then t.fresh_exit_ms=clock();timing(t,caller or "first_fresh",ok and s and "snapshot_passed" or "snapshot_failed",play)end
        if not ok then return nil,"snapshot_exception" end
        if not s then return nil,reason or "snapshot_unavailable" end
        if eligible~=true then return nil,"snapshot_eligibility_unavailable" end
        s=copy(s)
        if not s then return nil,"snapshot_fields" end
        if not same(active.scope,s) then return nil,"scope_changed" end
        return s
    end
    local function mark(role,ctx,box_param)
        if not active then return end
        local n=native();if not n then return end
        local admitted,status=pcall(n.status)
        if not admitted or type(status)~="table" or status.active~=true
            or type(status.pending)~="number" or status.pending<1 or status.pending_role~=role then return end
        local s,reason=fresh("first_mark")
        if not s then stop(reason);return end
        local ok,p,b=pcall(function()return ctx:get(),box_param:get()end)
        if not ok or not p or not b or not p:IsValid() or not b:IsValid()
            or p:GetAddress()~=s.pawn.address or b:GetAddress()~=s.box.address then return end
        -- This marker is pending-only. A Lua POST after native POST remains unpaired.
        pcall(n.mark,{world=s.world.address,pawn=s.pawn.address,mesh=s.mesh.address,box=s.box.address,
            box_owner=s.box_owner.address,match_id=s.match_id,round=s.round,life=s.life,role=role,marker=0})
    end
    local function install(epoch)
        local paths={"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage",
            "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage"}
        for role,path in ipairs(paths)do
            if control_epoch~=epoch then return false end
            if hooks[role]=="uncertain" then return false end
            if not hooks[role] then
                -- A thrown/ambiguous submission might already have registered. Never retry it.
                hooks[role]="uncertain"
                local ok,first,second=pcall(api.register,path,function(ctx,...)
                    if not active then return end
                    -- Both local SDK signatures put Hit Box at formal input15.
                    -- Pinned UE4SS Blueprint POST is the SECOND callback; no third callback.
                    pcall(mark,role,ctx,select(15,...))
                end)
                if control_epoch~=epoch then return false end
                if not ok or not hook_id(first) or not hook_id(second) or first~=second then return false end
                hooks[role]=true
            end
        end
        return true
    end
    local out={}
    function out.command(arg)
        local command_enter=clock()
        if api.developer()~=true then api.log("BOXOBS refused developer mode required");return end
        if arg=="off" then stop();return end
        local peer,side,seconds=tostring(arg):match("^(%d+)%s+([rl])%s+(%d+)$")
        peer,seconds=tonumber(peer),tonumber(seconds)
        if not positive(peer) or not positive(seconds) or seconds>15 then
            api.log("BOXOBS refused use <peer> <r|l> <seconds1..15> or off");return
        end
        if active or pending then stop("re-enrollment") end
        control_epoch=control_epoch+1
        local epoch=control_epoch
        local n=native();if not n then api.log("BOXOBS unavailable native API");return end
        timing_id=timing_id+1
        local t={id=timing_id,command_enter_ms=command_enter}
        t.snapshot_enter_ms=clock()
        local ok,s,reason,play,eligible=pcall(api.snapshot,peer,side)
        t.snapshot_exit_ms=clock()
        if control_epoch~=epoch then return end
        if not ok then reason="snapshot_exception"
        elseif s then s=copy(s);if not s then reason="snapshot_fields" elseif eligible~=true then s=nil;reason="snapshot_eligibility_unavailable" end
        else reason=reason or "snapshot_unavailable" end
        if not ok or not s then api.log("BOXOBS refused reason=%s authority=false",reason);return end
        s.duration_ms,s.calls=seconds*1000,32
        t.begin_enter_ms=clock()
        local started,yes,token=pcall(n.prepare,s)
        t.begin_exit_ms=clock()
        if control_epoch~=epoch then return end
        if not started or yes~=true or not positive(token) then
            timing(t,"enrollment","begin_failed",play)
            api.log("BOXOBS unavailable enrollment=%s",tostring(token or yes));return
        end
        t.install_enter_ms=clock()
        local installed=install(epoch)
        t.install_exit_ms=clock()
        if control_epoch~=epoch then return end
        timing(t,"enrollment",installed and "installed" or "install_failed",play)
        if control_epoch~=epoch then return end
        if not installed then stop("Lua POST hook unavailable");return end
        if not t.install_exit_ms then stop("preparation_clock_unavailable");return end
        pending={scope=copy(s),token=token,cutoff=t.install_exit_ms,deadline=t.install_exit_ms+250,timing=t,epoch=epoch}
        api.log("BOXOBS prepared peer=%s side=%s seconds=%s wait_ms=250 eligible=false authority=false",tostring(peer),side,tostring(seconds))
    end
    function out.tick()
        if pending then
            local p=pending
            local now=clock()
            if pending~=p or control_epoch~=p.epoch then return end
            if not now or now<p.cutoff or now>p.deadline then stop("preparation_expired");return end
            local n=native();if not n then stop("native API unavailable");return end
            local ok,s,reason,play,eligible=pcall(api.snapshot,p.scope.peer,p.scope.side,true)
            if pending~=p or control_epoch~=p.epoch then return end
            s=ok and copy(s) or nil
            if not s then stop(ok and (reason or "snapshot_fields") or "snapshot_exception");return end
            if not same(p.scope,s) then stop("scope_changed");return end
            if eligible~=true then
                if eligible==false and reason=="playback_timeage" then return end
                stop("pending_eligibility_unavailable");return
            end
            local applied=type(play)=="table" and play.local_ms
            if type(applied)~="number" or applied~=applied or math.abs(applied)==math.huge then stop("playback_timestamp_unavailable");return end
            if applied<=p.cutoff then return end -- The original applied timestamp must advance after setup.
            p.timing.activate_enter_ms=clock()
            local activated,yes,why=pcall(n.activate,p.token,s)
            p.timing.activate_exit_ms=clock()
            if pending~=p or control_epoch~=p.epoch then return end
            if not activated or yes~=true then stop("activation_"..tostring(why or yes));return end
            pending=nil;active={scope=s,drained=0,timing=p.timing}
            timing(p.timing,"activation","activated",play)
            api.log("BOXOBS started peer=%s side=%s calls=32 qualified=unavailable_until_pair authority=false",tostring(s.peer),s.side)
            return
        end
        if not active then return end
        local n=native();if not n then active=nil;return end
        local current,reason=fresh("first_tick")
        if not current then stop(reason);return end
        drain(n)
        local ok,s=pcall(n.status)
        if not ok or type(s)~="table" or s.active~=true then
            api.log("BOXOBS finished reason=%s entries=%s unpaired=%s discarded=%s proven_thread_id=%s foreign_callbacks_process_total=%s same_thread_unavailable_process_total=%s unknown_thread_callbacks_process_total=%s last_refused_caller_admission=%s foreign_callback_targets_known=false authority=false",
                tostring(type(s)=="table" and s.reason or "status unavailable"),
                tostring(type(s)=="table" and s.entries or "unavailable"),
                tostring(type(s)=="table" and s.unmatched or "unavailable"),
                tostring(type(s)=="table" and s.discarded or "unavailable"),
                process_counter(s,"proven_thread_id"),process_counter(s,"foreign_callbacks_process_total"),
                process_counter(s,"same_thread_unavailable_process_total"),process_counter(s,"unknown_thread_callbacks_process_total"),admission(s))
            active=nil
        end
    end
    function out.drop()stop("world leave")end
    return out
end
return M
