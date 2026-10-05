-- Read-only capture adapter. Native object arguments are used synchronously only.
-- Caller supplies existing proven exact Passport/component readers + own ctx.
local A={}
local function id(o)
    if not o or not o:IsValid() then return nil end
    local w=o:GetWorld(); if not w or not w:IsValid() then return nil end
    return {name=o:GetFullName(),address=o:GetAddress(),world=w:GetFullName()..'@'..tostring(w:GetAddress())}
end
local function same(a,b) return a and b and a.name==b.name and a.address==b.address and a.world==b.world end
function A.make(d)
    assert(type(d.context)=='function' and type(d.find_identity)=='function'
        and type(d.passport)=='function' and type(d.components)=='function'
        and type(d.generation)=='function' and type(d.equal_passport)=='function')
    local function source(o)
        local identity=id(o); if not identity then return nil end
        local w=o:GetWorld()
        local held,ammo,fired=o['Is Held'],o['Is Ammo'],o['Being Fired']
        if type(held)~='boolean' or type(ammo)~='boolean' or type(fired)~='boolean' then return nil end
        return {identity=identity,world_identity=id(w),class=o:GetClass():GetFullName(),
            parent=id(o['Parent Actor']) or false,holder=id(o:GetOwner()) or false,
            held=held,is_ammo=ammo,being_fired=fired,
            passport=d.passport(o),components=d.components(o),module_generation=d.generation(o)}
    end
    local function safe(o,route)
        local good,r=pcall(function()
            local s=source(o); if not s then return nil end
            if route=='shot' then
                s.string_loaded=o['String Loaded']; s.bolt_loaded=o['Bolt Loaded']
                if type(s.string_loaded)~='boolean' or type(s.bolt_loaded)~='boolean' then return nil end
                s.ammo=source(o['Loaded Ammo'])
            end
            return s
        end)
        return good and r or nil
    end
    return {context=d.context,now_ms=d.now_ms,equal_passport=d.equal_passport,snapshot=safe,
        resolve_snapshot=function(identity)
            local good,r=pcall(function()
                local o=d.find_identity(identity)
                if not same(id(o),identity) then return nil end
                return source(o)
            end)
            return good and r or nil
        end}
end
return A
