-- Exact RGBA8 run-length encoding of the synchronous native RVP color getter.
-- The getter's typed TArray<FColor> inner is hard. Never unwrap the full vertex
-- data struct in Lua: it also contains SoftObjectPtr fields.
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local Array=dofile((src:match("^(.*)[/\\]") or ".").."/native_source_array.lua")
local M={MAX_LODS=16,MAX_VERTICES=1000000,MAX_RUNS=4096}
local function guard(env)if env.guard()~=true then error("native vertex scope changed",0)end end
function M.capture(env)
    local function phase(stage,edge,detail)if env.phase then env.phase(stage,edge,detail or {})end end
    local function scalar(value)
        local t=type(value)
        if t=="nil"or t=="number"or t=="boolean"then return tostring(value)end
        if t=="string"then
            local quoted=string.format("%q",value:sub(1,120))
            return #quoted<=120 and quoted or quoted:sub(1,120).."[truncated]"
        end
        return "<non-scalar>" -- never invoke a returned wrapper's formatter
    end
    local ok,result=pcall(function()
        phase("vertex","enter",{getter="VertexPaintFunctionLibrary.GetMeshComponentVertexColorsAtLOD_Wrapper"})
        local getter=env.lod_getter or "GetNumLODs"
        local context=env.lod_context or ""
        phase("vertex_lods","enter",{getter=getter,reason=context})
        guard(env);local called,lods=pcall(env.lods);guard(env)
        if not called or type(lods)~="number" or not math.tointeger(lods) or lods<1 or lods>M.MAX_LODS then
            local reason="native vertex LOD count unavailable: getter="..getter
                ..(called and " returned_type=" or " error_type=")..type(lods)
                ..(called and " returned_value=" or " error=")..scalar(lods)..context
            phase("vertex_lods","exit",{getter=getter,ok=false,reason=reason})
            error(reason,0)
        end
        phase("vertex_lods","exit",{getter=getter,ok=true,count=lods,reason=context})
        local out={}
        for lod=0,lods-1 do
            phase("vertex_count","enter",{getter="GetMeshComponentAmountOfVerticesOnLOD",lod=lod})
            guard(env);local expected=env.count(lod);guard(env)
            if type(expected)~="number" or not math.tointeger(expected) or expected<1 or expected>M.MAX_VERTICES then error("native vertex count unavailable",0)end
            phase("vertex_count","exit",{ok=true,count=expected,lod=lod})
            if env.reserve then env.reserve(expected)end
            phase("vertex_color_getter","enter",{getter="GetMeshComponentVertexColorsAtLOD_Wrapper",count=expected,lod=lod})
            local colors=env.colors(lod);guard(env)
            phase("vertex_color_getter","exit",{ok=true,count=expected,lod=lod})
            phase("vertex_copy","enter",{getter="FColor typed copy/RLE",count=expected,lod=lod})
            local runs,seen={},0
            local copied=Array.collect(colors,expected,env.array_kind or "return",{guard=function()guard(env)end,
                context=(env.array_context or "VertexPaintFunctionLibrary.GetMeshComponentVertexColorsAtLOD_Wrapper").." lod="..tostring(lod)},function(v)
                guard(env)
                local c={v.R,v.G,v.B,v.A}
                for index=1,4 do local channel=c[index]
                    if type(channel)~="number" or not math.tointeger(channel) or channel<0 or channel>255 then error("native RGBA8 color unavailable",0)end
                end
                if #c~=4 then error("native RGBA8 color incomplete",0)end
                seen=seen+1;if seen>expected then error("native vertex count changed",0)end
                local last=runs[#runs]
                if last and last.color[1]==c[1] and last.color[2]==c[2] and last.color[3]==c[3] and last.color[4]==c[4]then last.count=last.count+1
                else if #runs>=M.MAX_RUNS then error("native vertex color run bound",0)end;runs[#runs+1]={count=1,color=c}end
                guard(env)
                return true
            end)
            guard(env)
            if seen~=expected or #copied~=expected or env.count(lod)~=expected then error("native vertex capture changed",0)end
            out[#out+1]={lod=lod,vertex_count=expected,runs=runs}
            phase("vertex_copy","exit",{ok=true,count=seen,lod=lod})
        end
        guard(env);if env.lods()~=lods then error("native vertex LOD count changed",0)end
        phase("vertex","exit",{ok=true,count=lods})
        return out
    end)
    if not ok then return nil,result end;return result
end
return M
