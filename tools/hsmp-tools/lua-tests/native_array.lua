local A=dofile(T.path("mods/shared/native_array.lua"))
local first,second={},{}
local seen={}
A.each({{get=function()return first end},{get=function()return second end}},function(c,i)seen[i]=c end)
T.check(seen[1]==first and seen[2]==second,"copied native return array unwraps RemoteUnrealParam exactly once")
seen={}
A.each({ForEach=function(_,f)f(1,{get=function()return first end})end},function(c,i)seen[i]=c end)
T.check(seen[1]==first,"native property TArray path retains identical element identity")
T.check(not pcall(A.each,{{}},function()end),"malformed array cannot be mistaken for successful complete enumeration")
