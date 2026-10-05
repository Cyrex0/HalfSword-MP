-- Original native melee modules and their cutting children. Used synchronously
-- by capture, pose production, and replay; no UObject is cached here.
local M={}
local function valid(o)return o and o:IsValid()end
local function same(a,b)
    if not valid(a) or not valid(b) then return false end
    if rawequal(a,b) then return true end
    local ok,result=pcall(function()local aa,bb=a:GetAddress(),b:GetAddress();return aa~=nil and aa==bb end)
    return ok and result or false
end
local function each(a,f)
    if a.ForEach then a:ForEach(function(_,e)f(e:get())end)
    else for _,v in ipairs(a)do f(v:get())end end
end
function M.of(weapon)
    local ok,rows,why=pcall(function()
        if not valid(weapon) then return nil,"invalid weapon" end
        local arr=weapon["Collision Components Array"]
        if not arr then return nil,"missing original collision array" end
        local parents,n={},0
        each(arr,function(c)
            n=n+1 -- Invalid native entries still consume their original ordinal.
            if valid(c) then parents[#parents+1]={component=c,id=n,child_of=0} end
        end)
        if n>15 then return nil,"collision ordinal capacity exceeded" end
        local out={}
        for _,r in ipairs(parents)do out[#out+1]=r end
        local nextid=n
        local seen={}
        for _,r in ipairs(parents)do
            local output={}
            -- Pinned UE4SS e3ba1016 copies FArray out parameters into this
            -- Lua table as RemoteUnrealParam entries. This function is void;
            -- each entry must be unwrapped before touching its UObject.
            r.component:GetChildrenComponents(true,output)
            local selected
            each(output,function(c)
                if valid(c) and same(c:GetOwner(),weapon) and not c:ComponentHasTag(FName("Tip")) then
                    local cls=c:GetClass()
                    if valid(cls) and cls:GetFName():ToString()=="BoxComponent" then selected=c end
                end
            end)
            local actual=false
            if selected then for _,p in ipairs(parents)do if same(p.component,selected) then actual=true end end end
            local duplicate=false
            if selected then for _,p in ipairs(seen)do if same(p,selected) then duplicate=true end end end
            if selected and not actual and not duplicate then
                local ancestor=selected:GetAttachParent()
                local ownerordinal
                for _=1,64 do
                    if not valid(ancestor) then break end
                    for _,p in ipairs(parents)do if same(p.component,ancestor) then ownerordinal=p.id;break end end
                    if ownerordinal then break end
                    ancestor=ancestor:GetAttachParent()
                end
                if not ownerordinal then return nil,"cutting child has no original module ancestor" end
                nextid=nextid+1
                out[#out+1]={component=selected,id=nextid,child_of=ownerordinal}
                seen[#seen+1]=selected
            end
        end
        if nextid>15 then return nil,"complete module/cutting-child ordinal capacity exceeded" end
        return out
    end)
    if not ok then return nil,"native module traversal unavailable: "..tostring(rows) end
    return rows,why
end
function M.box(weapon,source,box)
    local rows,why=M.of(weapon)
    if not rows then return nil,why end
    local parent
    for _,r in ipairs(rows)do if r.child_of==0 and same(r.component,source) then parent=r.id end end
    if not parent then return nil,"original source not an actual collision module" end
    for _,r in ipairs(rows)do
        if same(r.component,box) and same(weapon["Hit Box Collision"],box) then return r.id end
    end
    return nil,"original cutting Box is not the selected child of this source module"
end
function M.find(weapon,id,parent)
    local rows,why=M.of(weapon)
    if not rows then return nil,why end
    for _,r in ipairs(rows)do if r.id==id then return r.component end end
    return nil,"historical cutting Box parent identity unavailable"
end
return M
