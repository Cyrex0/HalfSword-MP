-- Pure lifecycle. Readiness is a successful native create/apply/readback, never DTO receipt.
local M={}
function M.new(env)
    local self={state="boot",seq=0,scope=nil,reported_at=-1e9,stopped=false}
    local function report(state,reason,scene,own)
        self.state=state
        env.report(state,reason,scene,own)
        self.reported_at=env.now()
    end
    function self:stop(reason)
        if self.stopped then return end
        self.stopped=true
        env.clear()
        env.close()
        report("stopped",reason)
    end
    function self:tick()
        if self.stopped then return false end
        local link=env.link()
        if not link or not link.connected then
            if self.scope then env.clear();self.scope=nil end
            if link and link.error and link.error~="" and link.error~="no answer from the server" then
                report("error",link.error);self:stop(link.error);return false
            end
            if self.state~="boot" then report("connected", "waiting for authenticated connection") end
            return false
        end
        local directory=env.directory()
        if not directory then return false end
        if directory.state==5 then report("error",directory.error);self:stop(directory.error);return false end
        local world=env.world()
        if not world or not world.ready then return false end
        if self.travel_pending then
            if world.key==self.travel_pending.key then
                if env.now()-self.travel_pending.at>30 then self:stop("client travel timed out")end
                return false
            end
            self.travel_pending=nil
            if world.arena~=directory.arena then self:stop("client arrived in the wrong source arena");return false end
            if not env.isolated()then self:stop("local authority suppression refused after travel");return false end
        end
        local isolated=world.arena==directory.arena and env.isolated()
        if world.arena~=directory.arena or not isolated then
            if not env.travel or not env.travel(directory.arena) then report("error","client arena isolation refused");self:stop("travel refused");return false end
            self.scope=nil;self.travel_pending={key=world.key,at=env.now()};report("travel",nil);return false
        end
        local scene=env.scene()
        if not scene then
            if self.scope then env.clear();self.scope=nil end
            if self.state~="wait_scene" then report("wait_scene","waiting for complete source recipes and frame") end
            return false
        end
        if scene.epoch~=directory.epoch or scene.dir_seq~=directory.seq then return false end
        local own
        for _,entity in ipairs(scene.entities or {})do if entity.owner_peer==scene.peer_id and entity.kind==0 then own=entity;break end end
        if not own then report("error","authenticated human binding unavailable");self:stop("missing player binding");return false end
        local key=tostring(own.epoch)..":"..tostring(own.id)..":"..tostring(own.incarnation)..":"..tostring(scene.dir_seq)..":"..tostring(world.key)
        if self.scope~=key then env.clear();self.scope=key end
        local applied,why=env.present()
        if applied~=true and why=="client mirror generation"then env.clear();self.scope=nil;report("wait_scene",why);return false end
        if applied~=true then report("error",why or "source-complete mirror refused",scene,own);self:stop(why or "mirror refused");return false end
        if type(why)=="table"then
            scene=why;own=nil
            for _,entity in ipairs(scene.entities or {})do if entity.owner_peer==scene.peer_id and entity.kind==0 then own=entity;break end end
            if not own then self:stop("applied scene has no owned human");return false end
        end
        local state=scene.state==2 and "live" or "mirror_ready"
        if self.state~=state or env.now()-self.reported_at>=1 then report(state,nil,scene,own)end
        if scene.state==2 then
            local axes,buttons=env.input(scene,own)
            if not axes then self:stop(buttons or "input unavailable");return false end
            self.seq=self.seq+1
            if self.seq>0xffffffff then self:stop("input counter exhausted");return false end
            local sent,reason=env.send({epoch=own.epoch,id=own.id,incarnation=own.incarnation,seq=self.seq,axes=axes,buttons=buttons})
            if sent~=true and reason~="native source or mirrors not ready" then self:stop(reason or "input refused");return false end
        end
        return true
    end
    return self
end
return M
