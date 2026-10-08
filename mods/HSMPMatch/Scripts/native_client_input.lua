-- Actual engine mappings -> raw held inputs. No local Willie is possessed or dispatched.
local M={}
local axes={"Move Forward / Backward","Move Right / Left","Turn Right / Left Mouse","Look Up / Down Mouse","Right Guard Axis","Left Guard Axis","Right Arm Axis","Left Arm Axis"}
local actions={"Run","Crouch Hold","Thrust","Jump","Grab Right","Grab Left","Talk"}
local function text(v)if type(v)=="string"then return v end;return v:ToString()end
function M.mapping(settings,each)
    local keys,index={},{}
    local function key(name)
        if not index[name]then if #keys>=96 then error("native input key bound")end;keys[#keys+1]=name;index[name]=#keys end
        return index[name]
    end
    local map={axes={},actions={},keys=keys}
    for i=1,8 do map.axes[i]={}end
    for i=1,7 do map.actions[i]={}end
    each(settings.AxisMappings,function(m)
        local name=text(m.AxisName)
        for i,want in ipairs(axes)do if name==want then
            local scale=tonumber(m.Scale);if not scale or scale~=scale then error("native axis mapping scale")end
            map.axes[i][#map.axes[i]+1]={key=key(text(m.Key.KeyName)),scale=scale}
        end end
    end)
    each(settings.ActionMappings,function(m)
        local name=text(m.ActionName)
        for i,want in ipairs(actions)do if name==want then
            local row={key=key(text(m.Key.KeyName)),mods={}}
            for _,v in ipairs({{"bShift","LeftShift","RightShift"},{"bCtrl","LeftControl","RightControl"},{"bAlt","LeftAlt","RightAlt"},{"bCmd","LeftCommand","RightCommand"}})do
                if type(m[v[1]])~="boolean"then error("native action modifier unavailable")end
                if m[v[1]]then row.mods[#row.mods+1]={key(v[2]),key(v[3])}end
            end
            map.actions[i][#map.actions[i]+1]=row
        end end
    end)
    for i=1,8 do if #map.axes[i]==0 then return nil,"native axis mapping unavailable: "..axes[i]end end
    for i=1,7 do if #map.actions[i]==0 then return nil,"native action mapping unavailable: "..actions[i]end end
    return map
end
function M.frame(map,values)
    if #values~=#map.keys then return nil,"native engine key state count"end
    local result,buttons={},0
    for i=1,8 do
        local value=0;for _,m in ipairs(map.axes[i])do value=value+values[m.key][1]*m.scale end
        local lo,hi=-1,1;if i==3 or i==4 then lo,hi=-100,100 elseif i==5 or i==6 then lo,hi=0,1 end
        result[i]=math.max(lo,math.min(hi,value))
    end
    for i=1,7 do for _,m in ipairs(map.actions[i])do
        local held=values[m.key][2]==1
        for _,mod in ipairs(m.mods)do held=held and (values[mod[1]][2]==1 or values[mod[2]][2]==1)end
        if held then buttons=buttons|(1<<(i-1));break end
    end end
    return result,buttons
end
-- The lab AI supplies intent through the same validated raw-input packet path.
-- It never installs a controller on a visual mirror or simulates local impacts.
function M.ai(scene,own,now)
    local other
    for _,e in ipairs(scene.entities or {})do if e.id~=own.id and e.kind==0 and e.position then other=e;break end end
    if not other or not own.position then return {0,0,0,0,0,0,0,0},0 end
    local dx,dy=other.position[1]-own.position[1],other.position[2]-own.position[2]
    local distance=math.sqrt(dx*dx+dy*dy)
    local turn=0
    if type(own.look_yaw)=="number"then
        local wanted=math.deg(math.atan(dy,dx));local difference=(wanted-own.look_yaw+180)%360-180
        turn=math.max(-3,math.min(3,difference/20))
    end
    local axes={distance>140 and 0.65 or 0,0,turn,0,0,0,0,0}
    local buttons=distance<220 and math.floor(now*5)%4==0 and (1<<2) or 0
    return axes,buttons
end
return M
