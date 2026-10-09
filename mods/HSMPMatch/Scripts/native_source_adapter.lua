-- Actual native static passport/gear capture. The caller supplies the worker's
-- fresh binding resolver and native bulk component collector. No body/gear CDO,
-- fallback passport, guessed mesh, or material/collision default is used.
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local dir=src:match("^(.*)[/\\]") or "."
local Descriptor=dofile(dir.."/native_source_descriptor.lua")
local Render=dofile(dir.."/native_source_render.lua")
local M={}
local binding_fields={"index","world_key","pc_address","pc_name","pawn_address","pawn_name"}
local sheath_fields={"Weapon Slot R 1","Weapon Slot R 2","Weapon Slot Back","Weapon Slot L 1","Weapon Slot L 2"}
local condition_fields={"HeadHealth_2_61859BB444171EF8952E0FA5DD8628EE","NeckHealth_4_C658DC6A4BD1988C40F1A5B3C4F8F4EE",
    "ArmRHealth_9_A65DD4C14ACBF6030A2B3AAD90FD0CFD","ArmLHealth_11_32345C31454A51B3CDE618918B9574F6",
    "BodyUpperHealth_16_F71EA0C742135DC3B4F71EA3FEF07C46","BodyLowerHealth_18_37C008FF4FA0C0E5F5E09C9F0C174FE3",
    "LegRHealth_13_D50D4E174859A541DBEA66963D162E12","LegLHealth_15_41C766B5460596C0804EA5B4B8F8EB36"}
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(binding_fields)do if a[k]==nil or a[k]~=b[k]then return false end end
    return true
end
function M.new(opts)
    assert(type(opts)=="table" and type(opts.resolve)=="function" and opts.WG,"source adapter dependencies")
    local api={}
    local function capture(index,phase)
        if not opts.WG.check() or not opts.WG.settled()then return nil,"source world settling"end
        local token=opts.WG.token();local expected=opts.resolve(index)
        if not expected or not opts.WG.same(token)then return nil,"source binding unavailable"end
        local bindings={pawn=expected.pawn_address,controller=expected.pc_address,components={},weapons={}}
        local function current()
            if not opts.WG.same(token)then error("source world changed",0)end
            local fresh=opts.resolve(index)
            if not same(expected,fresh) or not opts.WG.same(token)then error("source incarnation changed",0)end
            return fresh
        end
        local function read(fn)
            local fresh=current();local value=fn(fresh)
            current();return value
        end
        local function pawn_field(name)return read(function(b)return b.pawn[name]end)end
        bindings.world=read(function(b)return b.world:GetAddress()end)
        local function class_path(cls)
            if cls==nil then return ""end -- reflected hard class property: native null
            local address=cls:GetAddress()
            if address==0 then return ""end
            if cls:IsValid()~=true then error("source asset invalid",0)end
            local full=cls:GetFullName()
            local path=type(full)=="string" and full:match("^%S+%s+(.+)$")
            if not path or #path>512 or path:find("\0",1,true)then error("source asset path unavailable",0)end
            return path -- exact full identity; no shortening, _C removal or hashing
        end
        local env={guard=function()return pcall(current)end,phase=phase,
            unwrap=function(v)
                if type(v)=="number" or type(v)=="boolean" or type(v)=="string"then return v end
                return v:get() -- only SDK-proven hard map/array inner types
            end,
            name=function(v)return type(v)=="string" and v or v:ToString()end,
            class_path=class_path,
            character=function()return pawn_field("Character Passport")end,
            actor_class=function()return read(function(b)return class_path(b.pawn:GetClass())end)end,
            team=function()return pawn_field("Team Int")end,
            current_armor=function()return pawn_field("Currently Equipped Armor")end,
        }
        local function vector(fn)
            local out={};for i,k in ipairs({"X","Y","Z"})do out[i]=read(function(b)local v=fn(b);if not v then error("source scale unavailable",0)end;return v[k]end)end
            return out
        end
        env.construction=function()
            local condition={};for i,k in ipairs(condition_fields)do condition[i]=read(function(b)return b.pawn["Start Body Condition"][k]end)end
            return {start_body_condition=condition,is_zombie=pawn_field("Is Zombie?"),scale_mutation_inhibitor=pawn_field("Scale Mutation Inhibitor"),
                spawn_in_pants=pawn_field("Spawn in Pants"),bolts_in_quiver=pawn_field("Bolts in Quiver"),blossfechten_gear=pawn_field("Blossfechten Gear"),
                actor_scale=vector(function(b)return b.pawn:GetActorScale3D()end),character_scale=vector(function(b)return b.pawn["Character Scale (Set in BP)"]end),
                height_rate=pawn_field("Height Rate"),muscle_rate=pawn_field("Muscle Rate"),mass_scale=pawn_field("Mass Scale (Set in BP)")}
        end
        local function weapon(field,identity)
            local actor=pawn_field(field)
            if actor==nil then return nil end
            local address=actor:GetAddress();if address==0 then return nil end
            if actor:IsValid()~=true then error("source weapon invalid",0)end
            local name=actor:GetFName():ToString()
            if identity and (identity.address~=address or identity.name~=name)then error("source weapon changed",0)end
            local world=actor:GetWorld();current()
            if not world or world:IsValid()~=true or world:GetAddress()~=current().world:GetAddress()then error("source weapon world changed",0)end
            actor=pawn_field(field) -- GetWorld can reenter; obtain the exact weapon again
            if not actor or actor:GetAddress()~=address or actor:GetFName():ToString()~=name then error("source weapon changed",0)end
            return actor,{address=address,name=name,field=field}
        end
        env.live_weapons=function()
            local out={weapons={},hands={},sheaths={}};local seen={};bindings.weapons={}
            local function add(field,hand)
                local actor,identity=weapon(field);if not actor then return end
                local id=seen[identity.address]
                if not id then
                    id=#out.weapons+1;seen[identity.address]=id
                    local pass,why=Descriptor.read_passport("weapon",function()local latest=weapon(field,identity);return latest and latest["Weapon Passport"]end,env)
                    if not pass then error(why,0)end
                    local actor_class=read(function()local latest=weapon(field,identity);return class_path(latest:GetClass())end)
                    out.weapons[id]={id=id,actor_class=actor_class,passport=pass,components={}}
                    bindings.weapons[id]={id=id,address=identity.address,name=identity.name,field=field}
                end
                if hand~=nil then out.hands[#out.hands+1]={slot=hand,item=id}
                else out.sheaths[#out.sheaths+1]={field=field,item=id}end
            end
            add("Weapon R",0);add("Weapon L",1)
            for _,field in ipairs(sheath_fields)do add(field)end
            return out
        end
        env.render=function()
            local fresh=current()
            local captured,why
            if opts.capture_render then captured,why=opts.capture_render(fresh,bindings)
            else captured=Render.capture({read=read,guard=function()current()end,weapon=weapon,vertex_state=opts.vertex_state,
                phase=function(stage,edge,detail)detail=detail or {};detail.pass=env.pass;phase(stage,edge,detail)end,
                -- Pure copied FColor reads invoke no engine function. Fast token
                -- checks inside that loop avoid a controller search per vertex;
                -- every native getter still resolves the original binding.
                token_valid=function()return opts.WG.key==token.key and opts.WG.drops==token.drops and opts.WG.travel_from==nil end},bindings)end
            current();if not captured then error(why or "native render descriptor incomplete",0)end
            if type(captured.bindings)~="table"then error("native component bindings incomplete",0)end
            bindings.components=captured.bindings
            -- Persistent detached components must come from the bulk same-world,
            -- owner/weak qualified census, never the latest-cut scratch maps.
            local parts,err=Descriptor.read_flags(function()return pawn_field("Dismembered Parts Map")end,15,env)
            if not parts then error(err,0)end
            local bones,reason=Descriptor.read_names(function()return pawn_field("Dismembered Bones")end,512,env)
            if not bones then error(reason,0)end
            local topology=captured.topology
            if type(topology)~="table" or type(topology.detached)~="table" or type(topology.gore)~="table"
                or type(topology.vertex_state)~="string"then error("native persistent topology census incomplete",0)end
            topology.parts,topology.dismembered_bones,topology.in_process=parts,bones,pawn_field("Dismemberment In Process")
            return {components=captured.components,topology=topology}
        end
        local recipe,why=Descriptor.capture(env)
        if not recipe then return nil,why end
        if not pcall(current)then return nil,"source binding changed after descriptor capture"end
        return recipe,bindings
    end
    function api.capture(index,context)
        local token=opts.WG.token()
        local function phase(stage,edge,detail)
            -- Logging reads only the original token's cached scalar state.
            -- WG.same performs native lookups; never add one just for a phase.
            if opts.phase and context and opts.WG.key==token.key and opts.WG.drops==token.drops and opts.WG.travel_from==nil then
                opts.phase(context,stage,edge,detail or {})
            end
        end
        local ok,recipe,bindings=pcall(function()
            phase("adapter_capture","enter",{getter="SourceAdapter.capture"})
            return capture(index,phase)
        end)
        local completed,phase_reason=pcall(phase,"adapter_capture","exit",{ok=ok and recipe~=nil,
            reason=not ok and tostring(recipe) or recipe==nil and tostring(bindings) or nil})
        if not completed then return nil,phase_reason end
        if not ok then return nil,recipe end
        return recipe,bindings
    end
    return api
end
return M
