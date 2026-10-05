-- Compare exact wire values, never rounded shape signatures.
local E={}
function E.signature(record,fields)
    if type(record)~="table"then return "-"end
    local parts={}
    local function text(v)
        v=tostring(v or "");return string.pack("<I4",#v)..v
    end
    for _,fd in ipairs(fields)do
        local kind,v=fd[2],record[fd[3]]
        if kind=="class"or kind=="name"then parts[#parts+1]=text(v)
        elseif kind=="int"then parts[#parts+1]=string.pack("<i4",v or 0)
        elseif kind=="num"then parts[#parts+1]=string.pack("<f",v or 0)
        elseif kind=="vec"or kind=="color"then
            for i=1,(kind=="vec"and 3 or 4)do
                local n=type(v)=="table"and v[i]
                parts[#parts+1]=string.pack("<f",n or ((kind=="color"and i==4)and 1 or 0))
            end
        elseif kind=="bool"then parts[#parts+1]=(v==true or v==1)and "1"or "0"
        else error("unknown Passport field type")end
    end
    return (table.concat(parts):gsub(".",function(c)return string.format("%02x",c:byte())end))
end
local function float_equal(a,b)
    if type(a)~="number"or type(b)~="number"or a~=a or b~=b
        or math.abs(a)==math.huge or math.abs(b)==math.huge then return false end
    return string.pack("<f",a)==string.pack("<f",b)
end
function E.matches(pass,wanted,fields,class_path)
    if not pass or type(wanted)~="table"then return false end
    for _,fd in ipairs(fields)do
        local name,kind,key=fd[1],fd[2],fd[3]
        local read,value=pcall(function()return pass[name]end)
        if not read then return false end
        local target=wanted[key]
        if kind=="class"then
            local ok,path=pcall(class_path,value)
            if not ok or path~=(target or "")then return false end
        elseif kind=="name"then
            local ok,text=pcall(function()return value:ToString()end)
            if not ok or text~=(target or "")then return false end
        elseif kind=="int"then
            if type(value)~="number"or value~=math.tointeger(value)or value~=(target or 0)then return false end
        elseif kind=="num"then
            if not float_equal(value,target or 0)then return false end
        elseif kind=="vec"or kind=="color"then
            local components=kind=="vec"and {"X","Y","Z"}or {"R","G","B","A"}
            for i,c in ipairs(components)do
                local ok,n=pcall(function()return value[c]end)
                local expected=type(target)=="table"and target[i]
                if expected==nil then expected=(kind=="color"and i==4)and 1 or 0 end
                if not ok or not float_equal(n,expected)then return false end
            end
        elseif kind=="bool"then
            if type(value)~="boolean"or value~=(target==true or target==1)then return false end
        else return false end
    end
    return true
end
return E
