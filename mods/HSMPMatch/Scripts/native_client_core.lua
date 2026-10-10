-- Pure lifecycle. Readiness is a successful native create/apply/readback, never DTO receipt.
local M={}
function M.new(env)
    local self={state="boot",seq=0,scope=nil,binding=nil,reported_at=-1e9,stopped=false}
    local function report(state,reason,scene,own)
        self.state=state
        env.report(state,reason,scene,own)
        self.reported_at=env.now()
    end
    local function clear_scope()
        self.scope=nil;self.binding=nil;env.clear()
    end
    local function waiting(reason,scene,own)
        if self.state~="wait_scene" or env.now()-self.reported_at>=1 then report("wait_scene",reason,scene,own)end
        return false
    end
    local function owned(scene)
        for _,entity in ipairs(scene.entities or {})do
            if entity.owner_peer==scene.peer_id and entity.kind==0 then return entity end
        end
    end
    local function scene_key(scene,own,world)
        return scene.generation..":"..tostring(own.epoch)..":"..tostring(own.id)..":"..tostring(own.incarnation)..":"..tostring(world.key)
    end
    local function qualify(scene,directory,world)
        if scene.epoch~=directory.epoch or scene.dir_seq~=directory.seq then return nil,"client mirror generation"end
        if type(scene.generation)~="string" or scene.generation==""then return nil,"complete native scene generation unavailable"end
        if type(scene.fresh)~="boolean"then return nil,"native scene freshness unavailable"end
        local own=owned(scene)
        if not own or own.epoch~=scene.epoch then return nil,"authenticated human binding unavailable"end
        return own,scene_key(scene,own,world)
    end
    function self:stop(reason)
        if self.stopped then return end
        self.stopped=true
        local cleared,clear_reason=pcall(clear_scope)
        local closed,close_reason=pcall(env.close)
        report("stopped",reason or(not cleared and tostring(clear_reason))or(not closed and tostring(close_reason)))
    end
    function self:tick()
        if self.stopped then return false end
        local link=env.link()
        if not link or not link.connected then
            if self.scope then clear_scope()end
            if link and link.error and link.error~="" and link.error~="no answer from the server" then
                report("error",link.error);self:stop(link.error);return false
            end
            if self.state~="boot" then report("connected", "waiting for authenticated connection") end
            return false
        end
        local directory=env.directory()
        if not directory then return waiting("waiting for source directory")end
        if directory.state==5 then report("error",directory.error);self:stop(directory.error);return false end
        local world=env.world()
        if self.binding and (not world or world.key~=self.binding.world or directory.epoch~=self.binding.epoch or directory.seq~=self.binding.dir_seq)then clear_scope()end
        if not world or not world.ready then return false end
        if self.travel_pending then
            if world.key==self.travel_pending.key then
                if env.now()-self.travel_pending.at>30 then self:stop("client travel timed out")end
                return false
            end
            self.travel_pending=nil
            if world.arena~=directory.arena then self:stop("client arrived in the wrong source arena");return false end
            local isolated,why=env.isolated()
            if not isolated then self:stop(why or"local authority suppression refused after travel");return false end
        end
        local isolated,isolation_reason
        if world.arena==directory.arena then isolated,isolation_reason=env.isolated()end
        if world.arena~=directory.arena or not isolated then
            if not env.travel or not env.travel(directory.arena) then report("error",isolation_reason or"client arena isolation refused");self:stop(isolation_reason or"travel refused");return false end
            if self.scope then clear_scope()end
            self.travel_pending={key=world.key,at=env.now()};report("travel",nil);return false
        end
        local scene=env.scene()
        if not scene then
            return waiting("waiting for complete source recipes and frame")
        end
        local own,key=qualify(scene,directory,world)
        if not own then
            if key=="client mirror generation"then return waiting(key)end
            report("error",key);self:stop(key);return false
        end
        if self.scope~=key then clear_scope();self.scope=key end
        self.binding={epoch=scene.epoch,dir_seq=scene.dir_seq,world=world.key}
        if scene.state==2 and scene.fresh~=true then return waiting("native applied scene is stale",scene,own)end
        local applied,why=env.present(scene)
        if applied~=true and why=="client mirror generation"then clear_scope();return waiting(why)end
        if applied~=true and (why=="no complete current source scene"or why=="no coherent native scene"or why=="native applied scene is stale"or why=="native scene assets loading"or
            (env.gameplay and (why=="native gameplay pawn preparing"or why=="native gameplay own native view target pending"or why=="native gameplay own camera manager pending")))then return waiting(why,scene,own)end
        if applied~=true then report("error",why or "source-complete mirror refused",scene,own);self:stop(why or "mirror refused");return false end
        if type(why)~="table"then self:stop("native applied scene unavailable");return false end
        scene=why
        if env.gameplay and scene.gameplay_proof~=true then self:stop("native gameplay complete native proof unavailable");return false end
        local current_directory,current_world=env.directory(),env.world()
        if not current_directory or not current_world or not current_world.ready then return waiting("applied scene scope unavailable")end
        if current_directory.state==5 then self:stop(current_directory.error);return false end
        if current_world.key~=world.key then clear_scope();return waiting("client world changed during native apply")end
        own,key=qualify(scene,current_directory,current_world)
        if not own then
            if key=="client mirror generation"then clear_scope();return waiting(key)end
            self:stop(key);return false
        end
        -- The native result identifies what passed readback, even if a newer
        -- same-generation frame arrived while it was applied.
        self.scope=key;self.binding={epoch=scene.epoch,dir_seq=scene.dir_seq,world=current_world.key}
        local state=scene.state==2 and "live" or "mirror_ready"
        if scene.state==2 then
            if current_directory.state~=2 then return waiting("native source or mirrors not ready",scene,own)end
            if scene.fresh~=true then return waiting("native applied scene is stale",scene,own)end
            local axes,buttons=env.input(scene,own)
            if not axes then
                if buttons=="native applied scene is stale"or buttons=="native source or mirrors not ready"then return waiting(buttons,scene,own)end
                self:stop(buttons or "input unavailable");return false
            end
            self.seq=self.seq+1
            if self.seq>0xffffffff then self:stop("input counter exhausted");return false end
            local sent,reason=env.send({epoch=own.epoch,id=own.id,incarnation=own.incarnation,seq=self.seq,axes=axes,buttons=buttons})
            if sent~=true then
                if reason=="native applied scene is stale"or reason=="native source or mirrors not ready"then return waiting(reason,scene,own)end
                self:stop(reason or "input refused");return false
            end
        end
        if self.state~=state or env.now()-self.reported_at>=1 then report(state,nil,scene,own)end
        return true
    end
    return self
end
return M
