-- Exact RGBA8 run-length encoding of the synchronous native RVP color getter.
-- The getter's typed TArray<FColor> inner is hard. Never unwrap the full vertex
-- data struct in Lua: it also contains SoftObjectPtr fields.
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local Array=dofile((src:match("^(.*)[/\\]") or ".").."/native_source_array.lua")
local M={MAX_LODS=16,MAX_VERTICES=1000000,MAX_RUNS=4096}
local function guard(env)if env.guard()~=true then error("native vertex scope changed",0)end end
function M.capture(env)
    local ok,result=pcall(function()
        guard(env);local lods=env.lods();guard(env)
        if type(lods)~="number" or not math.tointeger(lods) or lods<1 or lods>M.MAX_LODS then error("native vertex LOD count unavailable",0)end
        local out={}
        for lod=0,lods-1 do
            guard(env);local expected=env.count(lod);guard(env)
            if type(expected)~="number" or not math.tointeger(expected) or expected<1 or expected>M.MAX_VERTICES then error("native vertex count unavailable",0)end
            if env.reserve then env.reserve(expected)end
            local colors=env.colors(lod);guard(env)
            local runs,seen={},0
            local copied=Array.collect(colors,expected,env.array_kind or "return",{guard=function()guard(env)end},function(v)
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
        end
        guard(env);if env.lods()~=lods then error("native vertex LOD count changed",0)end
        return out
    end)
    if not ok then return nil,result end;return result
end
return M
