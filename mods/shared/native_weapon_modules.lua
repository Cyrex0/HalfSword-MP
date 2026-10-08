-- Original native melee modules and their cutting children. Used synchronously
-- by capture, pose production, and replay; no UObject is cached here.
-- Pose production calls it per sample, so the traversal uses module-level callbacks and
-- reused scratch tables; only the returned rows are new.
local M={}
local function valid(o)return o and o:IsValid()end
local function addr_eq(a,b)local aa,bb=a:GetAddress(),b:GetAddress();return aa~=nil and aa==bb end
local function same(a,b)
    if not valid(a) or not valid(b) then return false end
    if rawequal(a,b) then return true end
    local ok,result=pcall(addr_eq,a,b)
    return ok and result or false
end
local tip
local function tip_name() tip=tip or FName("Tip");return tip end
-- Traversal state (one traversal at a time; nothing here calls back into M.of).
local W,N,parents,selected
local output,seen={},{}
local function add_parent(c)
    N=N+1 -- Invalid native entries still consume their original ordinal.
    if valid(c) then parents[#parents+1]={component=c,id=N,child_of=0} end
end
local function parent_entry(_,e)add_parent(e:get())end
local function add_child(c)
    if valid(c) and same(c:GetOwner(),W) and not c:ComponentHasTag(tip_name()) then
        local cls=c:GetClass()
        if valid(cls) and cls:GetFName():ToString()=="BoxComponent" then selected=c end
    end
end
local function child_entry(_,e)add_child(e:get())end
local function each(a,entry,item)
    if a.ForEach then a:ForEach(entry)
    else for _,v in ipairs(a)do item(v:get())end end
end
local function traverse(weapon)
    if not valid(weapon) then return nil,"invalid weapon" end
    local arr=weapon["Collision Components Array"]
    if not arr then return nil,"missing original collision array" end
    W,N,parents=weapon,0,{}
    each(arr,parent_entry,add_parent)
    if N>15 then return nil,"collision ordinal capacity exceeded" end
    local out={}
    for _,r in ipairs(parents)do out[#out+1]=r end
    local nextid=N
    for k=#seen,1,-1 do seen[k]=nil end
    for _,r in ipairs(parents)do
        for k=#output,1,-1 do output[k]=nil end
        -- Pinned UE4SS e3ba1016 copies FArray out parameters into this
        -- Lua table as RemoteUnrealParam entries. This function is void;
        -- each entry must be unwrapped before touching its UObject.
        r.component:GetChildrenComponents(true,output)
        selected=nil
        each(output,child_entry,add_child)
        local sel=selected
        local actual=false
        if sel then for _,p in ipairs(parents)do if same(p.component,sel) then actual=true end end end
        local duplicate=false
        if sel then for _,p in ipairs(seen)do if same(p,sel) then duplicate=true end end end
        if sel and not actual and not duplicate then
            local ancestor=sel:GetAttachParent()
            local ownerordinal
            for _=1,64 do
                if not valid(ancestor) then break end
                for _,p in ipairs(parents)do if same(p.component,ancestor) then ownerordinal=p.id;break end end
                if ownerordinal then break end
                ancestor=ancestor:GetAttachParent()
            end
            if not ownerordinal then return nil,"cutting child has no original module ancestor" end
            nextid=nextid+1
            out[#out+1]={component=sel,id=nextid,child_of=ownerordinal}
            seen[#seen+1]=sel
        end
    end
    if nextid>15 then return nil,"complete module/cutting-child ordinal capacity exceeded" end
    return out
end
local function release()
    W,parents,selected=nil,nil,nil
    for k=#output,1,-1 do output[k]=nil end
    for k=#seen,1,-1 do seen[k]=nil end
end
function M.of(weapon)
    local ok,rows,why=pcall(traverse,weapon)
    release() -- no UObject outlives the call
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
