-- HSMPDump — triggers UE4SS's built-in dumpers once at startup, with no
-- manual clicks in the UE4SS GUI.
--
-- Writes:
--   ue4ss/UHTHeaderDump/            (readable UCLASS/UPROPERTY headers)
--   ue4ss/CXXHeaderDump/            (C++ reflection headers in RC::Unreal ns)
--   ue4ss/UE4SS_ObjectDump.txt      (flat list of every UObject in GUObjectArray)
--   ue4ss/USMAP/                    (USMAP file for property-flagging tools)
--
-- Dumps are expensive and dependent on the game having loaded reflection
-- state. We defer them a few seconds after the engine's event loop starts
-- so GUObjectArray is populated and the main thread is idle.

local function Log(msg)
    print(string.format("[HSMPDump] %s\n", msg))
end

local already_dumped = false

local function RunDumps()
    if already_dumped then return end
    already_dumped = true
    Log("starting dump sequence")

    local t0 = os.clock()
    Log("GenerateUHTCompatibleHeaders() ...")
    local ok, err = pcall(GenerateUHTCompatibleHeaders)
    if not ok then Log("UHT error: " .. tostring(err)) end
    Log(string.format("UHT done in %.1fs", os.clock() - t0))

    local t1 = os.clock()
    Log("GenerateSDK() ...")
    local ok2, err2 = pcall(GenerateSDK)
    if not ok2 then Log("SDK error: " .. tostring(err2)) end
    Log(string.format("SDK done in %.1fs", os.clock() - t1))

    local t2 = os.clock()
    Log("DumpAllObjects() ...")
    local ok3, err3 = pcall(DumpAllObjects)
    if not ok3 then Log("Objects error: " .. tostring(err3)) end
    Log(string.format("Objects done in %.1fs", os.clock() - t2))

    local t3 = os.clock()
    Log("DumpUSMAP() ...")
    local ok4, err4 = pcall(DumpUSMAP)
    if not ok4 then Log("USMAP error: " .. tostring(err4)) end
    Log(string.format("USMAP done in %.1fs", os.clock() - t3))

    Log(string.format("all dumps complete in %.1fs total", os.clock() - t0))
end

Log("mod loaded; scheduling dump in ~8s")
ExecuteWithDelay(8000, RunDumps)
