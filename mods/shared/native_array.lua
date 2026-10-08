-- Pinned UE4SS e3ba1016: both native TArray.ForEach and copied UFunction
-- array outputs contain RemoteUnrealParam entries. Unwrap synchronously.
local M={}
function M.each(array,fn)
    if not array then error("native array unavailable")end
    if array.ForEach then
        array:ForEach(function(index,entry)fn(entry:get(),index)end)
    else
        for index,entry in ipairs(array)do fn(entry:get(),index)end
    end
end
return M
