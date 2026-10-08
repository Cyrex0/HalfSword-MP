-- DEV ONLY. No runtime adapter or production avatar integration is installed.
-- Every native operation below belongs to an explicitly proven trusted env.
local M={TOKEN="HSMP_Penetration_Quarantine",MAX_PER_WORLD=2}
local required={"full_passport_capture","deferred_passport_copy","passport_readback",
    "deferred_nonpossessing_spawn","finish_normal_beginplay","fresh_identity_resolve",
    "dormant_readback","native_readiness"}
local function copy(t,seen)
    local typ=type(t)
    if typ=="number" or typ=="boolean" or typ=="string" or typ=="nil" then return t end
    if typ~="table" then error("passport requires proven immutable plain snapshot") end
    seen=seen or {};if seen[t] then error("passport snapshot cycle")end;seen[t]=true
    local r={};for k,v in pairs(t)do r[copy(k,seen)]=copy(v,seen)end;seen[t]=nil;return r
end
local function close(a,b)
    return type(a)=="number" and type(b)=="number" and a==a and b==b
        and math.abs(a)<math.huge and math.abs(b)<math.huge
        and math.abs(a-b)<=math.max(.001,math.abs(b)*.001)
end
local function vector(a,b)
    return type(a)=="table" and type(b)=="table" and close(a[1],b[1])and close(a[2],b[2])and close(a[3],b[3])
end
local function world_same(a,b)
    return a and b and a.id==b.id and a.address==b.address and a.name==b.name
end
local function source_same(a,b)
    if type(a)~="table"or type(b)~="table"then return false end
    for _,k in ipairs({"name","address","world_id","match_id","round","life"})do
        if a[k]==nil or a[k]~=b[k]then return false end
    end
    return true
end
function M.new(env)
    local F={}
    local worlds,rows,next_id={},{},0
    local function current(row)
        local w=env.world()
        return world_same(w,row.world) and w or nil
    end
    local function actor(row)
        if not current(row) or not row.identity then return nil end
        local a=env.resolve(copy(row.identity),copy(row.world))
        if not a or not current(row)then return nil end
        local id=env.identity(a)
        if not id or id.name~=row.identity.name or id.address~=row.identity.address then return nil end
        return a
    end
    function F:gate()
        for _,name in ipairs(required)do
            if not env.proof or env.proof[name]~=true then return false,"unproved:"..name end
        end
        return true
    end
    function F:construct(source)
        local ready,why=self:gate();if not ready then return nil,why end
        local w=env.world()
        if not w or not w.id or not w.name or not w.address then return nil,"no native world"end
        local pool=worlds[w.id]
        if pool and not world_same(pool.world,w)then return nil,"world identity alias"end
        if not pool then
            for _,old in pairs(worlds)do
                if old.world.address==w.address and old.world.name==w.name then
                    -- A session/reconnect token cannot buy two more bodies.
                    -- Address reuse needs separately proven actual-world drop.
                    local ok,dropped=pcall(function()
                        return env.native_world_dropped and env.native_world_dropped(copy(old.world))==true
                    end)
                    if not ok or not dropped then return nil,"same native world has another token"end
                end
            end
        end
        if not pool then pool={world=copy(w),count=0};worlds[w.id]=pool end
        if pool.count>=M.MAX_PER_WORLD then return nil,"native world cap"end
        local ok,snap=pcall(env.capture,source)
        if not ok or not snap or snap.complete~=true or type(snap.passport)~="table"
            or type(snap.passport_hash)~="string" or snap.passport_hash=="" or not snap.baseline
            or not source_same(snap.source,source)or source.world_id~=w.id then
            return nil,"incomplete full passport capture"
        end
        local copied,immutable=pcall(copy,snap)
        if not copied then return nil,immutable end
        -- Reserve before spawn: partial/ambiguous native allocations also
        -- consume capacity, and neither destroy requests nor retries free it.
        pool.count=pool.count+1;next_id=next_id+1
        local row={id=next_id,token=M.TOKEN,world=copy(w),snapshot=immutable,state="reserved"}
        rows[row.id]=row
        if not current(row)then row.state="orphan";return row.id,"world changed before allocation"end
        local spawned,a=pcall(env.begin,copy(w),{player=false,auto_possess_player=0,
            auto_possess_ai=0,spawn_ai=false,hidden=true,collision=false,token=M.TOKEN})
        if not spawned or not a then row.state="orphan";return row.id,"partial spawn retained"end
        if not current(row)then row.state="orphan";return row.id,"world dropped after allocation"end
        local named,id=pcall(env.identity,a)
        if not named or not id or type(id.name)~="string" or type(id.address)~="number" then
            row.state="orphan";return row.id,"unresolved allocation retained"
        end
        if id.name==source.name and id.address==source.address then
            row.state="orphan";return row.id,"constructor returned source actor; untouched"
        end
        for _,other in pairs(rows)do
            if other~=row and other.identity and other.identity.name==id.name and other.identity.address==id.address
                and world_same(other.world,w)then row.state="orphan";return row.id,"duplicate allocation identity; untouched"end
        end
        row.identity={name=id.name,address=id.address};row.state="constructing"
        -- No tags setter is guessed. This private ledger alone owns the actor.
        local applied,err=pcall(function()
            if not current(row)then error("world changed before passport")end
            if env.apply_passport(a,copy(row.snapshot.passport),row.snapshot.passport_hash)~=true then error("passport copy failed")end
            if not current(row)then error("world changed before readback")end
            if env.passport_hash(a)~=row.snapshot.passport_hash then error("passport readback mismatch")end
            if not current(row)then error("world changed before finish")end
            if env.finish(a)~=true then error("finish failed")end
        end)
        if not applied then row.state="retired";self:retire(row.id);return row.id,tostring(err)end
        row.state="waiting" -- normal delayed BeginPlay only; no extra setup event
        return row.id
    end
    function F:poll(id)
        local row=rows[id]
        if not row or row.state~="waiting"then return false,"not waiting"end
        local located,a=pcall(actor,row);if not located or not a then return false,"no fresh owned actor"end
        local probed,p=pcall(env.probe,a);if not probed then return false,"native probe failed"end
        local b=row.snapshot.baseline
        if not p or p.body_count~=22 or p.native_joint_count~=21 or p.physics_asset~=b.physics_asset
            or not vector(p.actor_scale,b.actor_scale)or not vector(p.mesh_scale,b.mesh_scale)
            or not close(p.height,b.height)or p.motors_ready~=true or p.controls_ready~=true
            or p.tick_owned~=true or p.player~=false or p.ai_active~=false
            or p.hidden~=true or p.collision~=false then return false,"native readiness pending"end
        if type(p.masses)~="table" or type(b.masses)~="table" then return false,"mass evidence missing"end
        local n=0
        for bone,mass in pairs(b.masses)do n=n+1;if not close(p.masses[bone],mass)then return false,"mass mismatch:"..bone end end
        if n~=22 then return false,"22 source masses required"end
        row.state="ready";row.readback=copy(p)
        return true
    end
    function F:retire(id)
        local row=rows[id];if not row then return false,"not factory owned"end
        row.state="retired"
        local located,a=pcall(actor,row);if not located or not a then return false,"no current fresh owned actor"end
        -- Willie Destroy is a known no-op. Never invoke it or retain delayed
        -- UObject closures: retirement uses a fresh lookup every retry.
        local applied,done=pcall(env.dormant,a,{physics=false,collision=false,hidden=true,ai_active=false,tick=false})
        if not applied or done~=true then
            return false,"dormant application failed"
        end
        if not current(row)then return false,"world dropped after dormancy"end
        local probed,p=pcall(env.probe,a);if not probed then return false,"dormant probe failed"end
        if not p or p.physics~=false or p.collision~=false or p.hidden~=true
            or p.ai_active~=false or p.tick_owned~=false then return false,"dormant readback pending"end
        row.state="dormant";return true
    end
    function F:with_pair(aid,bid,probe)
        local arow,brow=rows[aid],rows[bid]
        if not env.proof or env.proof.owned_pair_control~=true then return false,"unproved:owned_pair_control"end
        if aid==bid or not arow or not brow or arow.state~="ready"or brow.state~="ready"
            or not world_same(arow.world,brow.world)then return false,"two ready owned bodies required"end
        local result
        local ok,err=pcall(function()
            local a,b=actor(arow),actor(brow)
            if not a or not b or not current(arow)or not current(brow)then error("pair lacks fresh owned identities")end
            if env.activate_pair(a,b,M.TOKEN)~=true then error("pair activation refused")end
            if not current(arow)or not current(brow)then error("world dropped during activation")end
            arow.state,brow.state="probing","probing"
            -- Synchronous bounded probe only: never schedule captured objects.
            result=copy(probe(a,b))
        end)
        local aclean=self:retire(aid);local bclean=self:retire(bid)
        if not ok then return false,tostring(err)end
        if not aclean or not bclean then return false,"probe ended; dormant cleanup pending"end
        return true,result
    end
    function F:owned(id)
        local row=rows[id]
        return row and {token=row.token,identity=copy(row.identity),world=copy(row.world),state=row.state}or nil
    end
    return F
end
return M
