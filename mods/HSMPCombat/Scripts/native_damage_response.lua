-- Native scalar out tables from the one existing owner DCD invocation.
-- UE4SS LuaUObject.cpp copies each scalar under its exact FProperty name.
-- Missing/partial copy-out is evidence unavailable, never a reason to retry.
local M={FIELDS={{18,"Hit Surface","byte"},{19,"Damage Out","number"},
    {20,"Cutting Rate Out","number"},{21,"Rigidity Out","number"},
    {22,"Material Density Out","number"},{23,"Lower Threshold Out","boolean"}}}
local function finite(v)return type(v)=="number"and v==v and math.abs(v)<math.huge end
function M.capture(args,call_ok,context)
    local row={authority=false,phase="owner_DCD_post_invocation",call_ok=call_ok==true,
        complete=call_ok==true,fields={},context={},context_available=true}
    for _,f in ipairs(M.FIELDS)do
        local t,v
        if type(args)=="table"then t=rawget(args,f[1])end
        if type(t)=="table"then v=rawget(t,f[2])end
        local good=f[3]=="boolean"and type(v)=="boolean"
            or f[3]=="number"and finite(v)
            or f[3]=="byte"and finite(v)and math.tointeger(v)and v>=0 and v<=255
        row.fields[f[2]]=good and {available=true,value=v}or {available=false,reason="scalar output unavailable"}
        if not good then row.complete=false end
    end
    for _,k in ipairs({"world","drops","match_id","round","attacker","attacker_life","victim","victim_life","hit_id"})do
        local v=type(context)=="table"and rawget(context,k)
        local good
        if k=="world"then good=type(v)=="string"and #v>0 and #v<=512 and not v:find("\0",1,true)
        else good=finite(v)and math.tointeger(v)and v>=(k=="drops"and 0 or 1)end
        if good then row.context[k]=v else row.context_available=false end
    end
    return row
end
return M
