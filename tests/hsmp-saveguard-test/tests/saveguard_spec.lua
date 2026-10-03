-- Scenarios for hsmp_saveguard.lua, driven by tests/saveguard.rs (mlua, Lua 5.4).
-- Each scenario runs in a FRESH Lua state with its own temp <state_dir> and
-- SaveGames dir (created by the Rust runner); the real SaveGames dir is never
-- touched. UE4SS is mocked: RegisterHook records callbacks, RemoteUnrealParam
-- is a table with get/set, FString has ToString/Clear/Append.
--
-- run(index, sg_src, state_dir, save_dir) -> name, { {check, ok, detail}... }

local SPEC = {}

-- --- harness -------------------------------------------------------------------

local function new_harness(sg_src, state_dir, save_dir, extra)
    local SG = assert(load(sg_src, "=hsmp_saveguard.lua"))()
    local H = { hooks = {}, events = {}, printed = {}, t = 1759340000.0, c = 10.0, gi_calls = 0,
                results = {}, SG = SG, state_dir = state_dir, save_dir = save_dir }

    function H.check(name, cond, detail)
        H.results[#H.results + 1] = { name, cond and true or false, detail and tostring(detail) or "" }
    end

    function H.FStr(s)
        local o = { s = s }
        function o:ToString() return self.s end
        function o:Clear() if H.no_clear then error("no Clear") end self.s = "" end
        function o:Append(x) self.s = self.s .. x end
        return o
    end

    -- flags: noset (set() throws), setnoop (set() silently ignored), noget
    function H.Param(v, flags)
        flags = flags or {}
        local p = { v = v }
        function p:get() if flags.noget then error("get failed") end return self.v end
        function p:set(n)
            if flags.noset then error("StrProperty can only be set to a string or FString") end
            if flags.setnoop then return end
            if type(n) == "string" then self.v = H.FStr(n) else self.v = n end
        end
        return p
    end

    function H.Obj() return { IsValid = function() return true end } end

    local opts = {
        mod = "TEST", inst = "3", state_dir = state_dir, save_dir = save_dir,
        log = { event = function(name, f) H.events[#H.events + 1] = { name = name, f = f } end },
        print = function(s) H.printed[#H.printed + 1] = s end,
        now = function() return H.t end,
        clock = function() return H.c end,
        -- the sidecar's typed link record (status name) + its header heartbeat age
        link = function()
            if H.lk_status == nil then return nil end
            return H.lk_status, H.lk_beat and (H.c - H.lk_beat) or nil
        end,
        register_hook = function(path, fn) H.hooks[path] = fn end,
        get_gi = function()
            return { IsValid = function() return true end,
                     ["Load Game"] = function(self) H.gi_calls = H.gi_calls + 1 end }
        end,
    }
    for k, v in pairs(extra or {}) do opts[k] = v end
    H.GS = "/Script/Engine.GameplayStatics:"
    H.AS = "/Script/Engine.AsyncActionHandleSaveGame:"

    function H.save(slot, flags)
        local po, ps = H.Param(H.Obj()), H.Param(H.FStr(slot), flags)
        H.hooks[H.GS .. "SaveGameToSlot"]({}, po, ps, H.Param(0))
        return ps.v and ps.v.s, po.v
    end
    function H.load(slot)
        local ps = H.Param(H.FStr(slot))
        H.hooks[H.GS .. "LoadGameFromSlot"]({}, ps, H.Param(0))
        return ps.v.s
    end
    function H.exists(slot)
        local ps = H.Param(H.FStr(slot))
        H.hooks[H.GS .. "DoesSaveGameExist"]({}, ps, H.Param(0))
        return ps.v.s
    end
    function H.delete(slot, flags)
        local ps = H.Param(H.FStr(slot), flags)
        H.hooks[H.GS .. "DeleteGameInSlot"]({}, ps, H.Param(0))
        return ps.v.s
    end
    function H.async_save(slot)
        local po, ps = H.Param(H.Obj()), H.Param(H.FStr(slot))
        H.hooks[H.AS .. "AsyncSaveGameToSlot"]({}, H.Param(H.Obj()), po, ps, H.Param(0))
        return ps.v.s
    end
    function H.async_load(slot)
        local ps = H.Param(H.FStr(slot))
        H.hooks[H.AS .. "AsyncLoadGameFromSlot"]({}, H.Param(H.Obj()), ps, H.Param(0))
        return ps.v.s
    end

    function H.count(name, pred)
        local n = 0
        for _, e in ipairs(H.events) do
            if e.name == name and (pred == nil or pred(e.f)) then n = n + 1 end
        end
        return n
    end
    function H.last(name)
        for i = #H.events, 1, -1 do if H.events[i].name == name then return H.events[i].f end end
    end
    function H.clear() H.events = {} end
    function H.tick(dt) dt = dt or 1.0; H.c = H.c + dt; H.t = H.t + dt end

    function H.write_file(path, data)
        local f = assert(io.open(path, "wb")); f:write(data); f:close()
    end
    function H.read_file(path)
        local f = io.open(path, "rb"); if not f then return nil end
        local s = f:read("a"); f:close(); return s
    end
    -- The sidecar: its link record says `status`; beat() = a fresh header heartbeat.
    function H.link(status) H.lk_status = status end
    function H.beat() H.lk_beat = H.c end
    function H.unlink() H.lk_status, H.lk_beat = nil, nil end
    -- a connected link with a beating sidecar -> live
    function H.go_live()
        H.link("connected"); H.beat(); H.tick(1); H.beat(); SG.refresh(true)
    end

    H.opts = opts
    return H
end

local function scenario(name, extra, fn) SPEC[#SPEC + 1] = { name = name, extra = extra, fn = fn } end

local function install(H) H.SG.install(H.opts); return H.SG end

-- --- scenarios ---------------------------------------------------------------------

scenario("install", nil, function(H)
    local SG = install(H)
    local n = 0; for _ in pairs(H.hooks) do n = n + 1 end
    H.check("6 hooks registered", n == 6, n)
    for _, p in ipairs({ "GameplayStatics:SaveGameToSlot", "GameplayStatics:LoadGameFromSlot",
                         "GameplayStatics:DoesSaveGameExist", "GameplayStatics:DeleteGameInSlot",
                         "AsyncActionHandleSaveGame:AsyncSaveGameToSlot",
                         "AsyncActionHandleSaveGame:AsyncLoadGameFromSlot" }) do
        H.check("hook /Script/Engine." .. p, H.hooks["/Script/Engine." .. p] ~= nil)
    end
    H.check("mapped slot name", SG.mapped("GameProgress") == "HSMP_3_GameProgress", SG.mapped("GameProgress"))
    local hk = SG.hooks(); local all = true
    for _, v in pairs(hk) do if v ~= true then all = false end end
    H.check("hook table reports success", all)
    local e = H.last("x_save_guard")
    H.check("DoD-8: install states the guard once (x_save_guard active=false)", e ~= nil and e.active == false
        and H.count("x_save_guard") == 1, e and e.why)
end)

scenario("install survives a failing RegisterHook", nil, function(H)
    H.opts.register_hook = function(path, fn)
        if path:find("Async") then error("function not found") end
        H.hooks[path] = fn
    end
    local SG = install(H)
    H.check("sync hooks still registered", H.hooks[H.GS .. "SaveGameToSlot"] ~= nil)
    H.check("failure recorded", type(SG.hooks().AsyncSaveGameToSlot) == "string")
    SG.set_active(true)
    H.check("still redirects", (H.save("GameProgress")) == "HSMP_3_GameProgress")
end)

scenario("no sidecar link: pass-through + spy", nil, function(H)
    install(H)
    local slot = H.save("GameProgress")
    H.check("save passes through", slot == "GameProgress", slot)
    H.check("spy logs x_save_call", H.count("x_save_call", function(f)
        return f.slot == "GameProgress" and f.active == false and f.fn == "SaveGameToSlot" end) == 1)
    H.check("no save_redirected", H.count("save_redirected") == 0)
end)

scenario("a connected link without a heartbeat never activates (dead sidecar)", nil, function(H)
    H.link("connected"); H.lk_beat = H.c - 600
    local SG = install(H)
    for _ = 1, 5 do H.tick(5); SG.refresh(true) end
    H.check("inactive", SG.is_active() == false, SG.reason())
    H.check("career save untouched", (H.save("GameProgress")) == "GameProgress")
end)

scenario("live session: redirect + copy-on-write", nil, function(H)
    local sv = H.save_dir
    H.write_file(sv .. "/HSMP_3_GameProgress.sav", "old-mp")   -- leftover from the last session
    H.write_file(sv .. "/HSMP_1_GameProgress.sav", "other-inst")
    H.write_file(sv .. "/GameProgress.sav", "career")
    local SG = install(H)
    H.go_live()
    H.check("connected + heartbeat -> active", SG.is_active() == true, SG.reason())
    H.check("activation purges this instance's HSMP_ slots", H.read_file(sv .. "/HSMP_3_GameProgress.sav") == nil)
    H.check("activation keeps other instances' HSMP_ slots", H.read_file(sv .. "/HSMP_1_GameProgress.sav") == "other-inst")
    H.check("activation never touches career files", H.read_file(sv .. "/GameProgress.sav") == "career")
    H.check("x_save_guard active event", H.count("x_save_guard", function(f) return f.active == true end) == 1)

    H.check("COW: load before write -> career", H.load("GameProgress") == "GameProgress")
    H.check("COW: exists before write -> career", H.exists("GameProgress") == "GameProgress")
    H.check("COW: career reads emit no save_redirected", H.count("save_redirected") == 0)

    local slot, obj = H.save("GameProgress")
    H.check("SaveGameToSlot rewritten", slot == "HSMP_3_GameProgress", slot)
    local e = H.last("save_redirected") or {}
    H.check("save_redirected{fn,slot,to_slot,ok,op}", e.fn == "SaveGameToSlot" and e.slot == "GameProgress"
        and e.to_slot == "HSMP_3_GameProgress" and e.ok == true and e.op == "write")
    H.check("save object untouched when redirected", obj ~= nil)
    H.check("COW: load after write -> HSMP", H.load("GameProgress") == "HSMP_3_GameProgress")
    H.check("COW: exists after write -> HSMP", H.exists("GameProgress") == "HSMP_3_GameProgress")
    H.check("COW: untouched slot still reads career", H.load("Settings") == "Settings")
    H.write_file(sv .. "/HSMP_3_Settings.sav", "x")
    H.check("COW: existing HSMP_ file -> read redirected", H.load("Settings") == "HSMP_3_Settings")
    H.check("HSMP_ slot passes through (no double prefix)", (H.save("HSMP_3_GameProgress")) == "HSMP_3_GameProgress")
    H.check("other instance's HSMP_ slot passes through", (H.save("HSMP_1_GameProgress")) == "HSMP_1_GameProgress")
    H.check("slot with spaces", (H.save("SG Gauntlet Progress")) == "HSMP_3_SG Gauntlet Progress")

    H.check("delete rewritten", H.delete("Save_Crash") == "HSMP_3_Save_Crash")
    H.check("tombstone: exists after delete -> HSMP", H.exists("Save_Crash") == "HSMP_3_Save_Crash")
    H.check("save after delete", (H.save("Save_Crash")) == "HSMP_3_Save_Crash")
    H.check("AsyncSaveGameToSlot rewritten", H.async_save("GameProgress") == "HSMP_3_GameProgress")
    H.check("AsyncLoadGameFromSlot COW", H.async_load("GameProgress") == "HSMP_3_GameProgress"
        and H.async_load("SG Player Equipment") == "SG Player Equipment")
    H.check("stats counted", SG.stats.redirected >= 8, SG.stats.redirected)
end)

scenario("param set() fallbacks and neutralised writes", nil, function(H)
    local SG = install(H)
    SG.set_active(true)
    H.check("set() throws -> FString Clear/Append", (H.save("Settings", { noset = true })) == "HSMP_3_Settings")
    H.check("set() no-op -> FString Clear/Append", (H.save("Settings", { setnoop = true })) == "HSMP_3_Settings")
    H.no_clear = true
    H.clear()
    local slot, obj = H.save("GameProgress", { noset = true })
    local e = H.last("save_redirected") or {}
    H.check("rewrite impossible -> SaveGameObject nulled", obj == nil and slot == "GameProgress", tostring(slot))
    H.check("neutralised write reported ok + blocked", e.ok == true and e.to_slot == "<blocked>"
        and tostring(e.how):find("blocked") ~= nil)
    H.clear()
    local d = H.delete("GameProgress", { noset = true })
    e = H.last("save_redirected") or {}
    H.check("undivertable delete reported ok=false", e.ok == false and d == "GameProgress")
    local warned = false
    for _, s in ipairs(H.printed) do if s:find("could NOT be diverted", 1, true) then warned = true end end
    H.check("failure warning printed", warned)
    H.no_clear = false
    local ok = pcall(H.hooks[H.GS .. "SaveGameToSlot"], {}, H.Param(H.Obj()), H.Param(nil, { noget = true }), H.Param(0))
    H.check("hook never raises on bad params", ok)
    H.check("bad param counted as failed", SG.stats.failed >= 2, SG.stats.failed)
end)

scenario("sticky window, going stale, career GI reload", { live_s = 15, sticky_s = 45 }, function(H)
    local SG = install(H)
    H.go_live()
    H.check("active", SG.is_active() == true)
    H.tick(30); SG.refresh(true)
    H.check("still active inside the sticky window", SG.is_active() == true, SG.reason())
    H.tick(30); SG.refresh(true)
    H.check("stale beyond the sticky window -> inactive", SG.is_active() == false, SG.reason())
    H.check("no GI reload before tick()", H.gi_calls == 0)
    SG.tick()
    H.check("tick(): GI 'Load Game' after the session", H.gi_calls == 1, H.gi_calls)
    SG.tick()
    H.check("GI reload happens once", H.gi_calls == 1)
    H.check("career save passes through again", (H.save("GameProgress")) == "GameProgress")
end)

scenario("terminal status", nil, function(H)
    local SG = install(H)
    H.go_live()
    H.link("rejected"); H.beat(); H.tick(1); SG.refresh(true)
    H.check("rejected -> inactive at once", SG.is_active() == false, SG.reason())
end)

scenario("sidecar link removed", nil, function(H)
    local SG = install(H)
    H.go_live()
    H.unlink(); H.tick(0.5); SG.refresh(true)
    H.check("one absent read is not proof of absence", SG.is_active() == true, SG.reason())
    H.tick(1); SG.refresh(true)
    H.check("gone for >= absent_s over 2 reads -> inactive", SG.is_active() == false, SG.reason())
end)

scenario("a heartbeat gap inside the sticky window is no session end", nil, function(H)
    local SG = install(H)
    local sv = H.save_dir
    H.go_live()
    H.check("active", SG.is_active() == true, SG.reason())
    H.check("one session", SG.state().sessions == 1)
    H.save("GameProgress")
    H.check("MP slot written", SG.state().written["HSMP_3_GameProgress"] == true)
    H.write_file(sv .. "/HSMP_3_GameProgress.sav", "mp-progress")
    H.clear()
    for i = 1, 6 do
        H.tick(5); SG.refresh(true)   -- no beat: 5..30 s old, live_s 15, sticky_s 45
        H.check("still active after a " .. (5 * i) .. " s heartbeat gap", SG.is_active() == true, SG.reason())
    end
    H.check("no x_save_guard inactive event", H.count("x_save_guard", function(f) return f.active == false end) == 0)
    H.go_live()
    H.check("still the same session (no re-activation)", SG.state().sessions == 1, SG.state().sessions)
    H.check("MP slot NOT wiped by a blip", H.read_file(sv .. "/HSMP_3_GameProgress.sav") == "mp-progress")
    H.check("no GI career reload queued", SG.state().pending_gi_reload == false)
    SG.tick()
    H.check("no GI career reload ran", H.gi_calls == 0)
end)

scenario("no heartbeat never activates, and the sticky window still expires", { sticky_s = 45 }, function(H)
    local SG = install(H)
    H.link("connecting"); SG.refresh(true)
    H.tick(1); SG.refresh(true)
    H.check("a link without any heartbeat stays inactive", SG.is_active() == false, SG.reason())
    H.go_live()
    H.check("active", SG.is_active() == true)
    for _ = 1, 70 do H.tick(1); SG.refresh(true) end   -- live_s 15 + sticky_s 45 + margin
    H.check("no heartbeat for > sticky_s -> inactive", SG.is_active() == false, SG.reason())
end)

scenario("kicked / replaced / server_closed are terminal", nil, function(H)
    for i, s in ipairs({ "kicked", "replaced", "server_closed" }) do
        H.SG.set_active(nil)
        if i == 1 then install(H) end
        H.go_live()
        H.check(s .. ": active first", H.SG.is_active() == true, H.SG.reason())
        H.link(s); H.beat(); H.tick(1); H.SG.refresh(true)
        H.check(s .. " -> inactive", H.SG.is_active() == false, H.SG.reason())
    end
end)

scenario("header heartbeat", nil, function(H)
    local SG = install(H)
    H.link("connecting"); H.beat(); SG.refresh(true)
    H.check("a fresh heartbeat -> active on the first read", SG.is_active() == true, SG.reason())
    H.link("connected"); H.lk_beat = H.c - 3600
    H.tick(60); SG.refresh(true)
    H.check("a stale heartbeat -> inactive", SG.is_active() == false, SG.reason())
end)

scenario("poll rate limit", { poll_s = 0.5 }, function(H)
    local SG = install(H)
    H.go_live()
    H.unlink()
    H.c = H.c + 0.1
    H.check("within poll_s the cached state is used", SG.is_active() == true)
    H.c = H.c + 1.0
    SG.is_active()
    H.c = H.c + 1.0
    H.check("after poll_s the link is re-read", SG.is_active() == false)
end)

scenario("Director override", nil, function(H)
    local SG = install(H)
    SG.set_active(true)
    H.check("set_active(true) redirects without a sidecar", (H.save("GameProgress")) == "HSMP_3_GameProgress")
    H.go_live()
    SG.set_active(false)
    H.check("set_active(false) wins over a live sidecar", (H.save("GameProgress")) == "GameProgress")
    SG.set_active(nil)
    H.go_live()
    H.check("set_active(nil) -> automatic", SG.is_active() == true, SG.reason())
end)

scenario("mode=block", { mode = "block" }, function(H)
    local SG = install(H)
    H.go_live()
    local slot, obj = H.save("GameProgress")
    H.check("slot untouched, object nulled", slot == "GameProgress" and obj == nil)
    H.check("reads stay on career", H.load("GameProgress") == "GameProgress")
    H.check("blocked counted", SG.stats.blocked == 1)
end)

scenario("passthrough slots", { passthrough = { Settings = true } }, function(H)
    local SG = install(H)
    SG.set_active(true)
    H.check("Settings passes through", (H.save("Settings")) == "Settings")
    H.check("GameProgress still redirected", (H.save("GameProgress")) == "HSMP_3_GameProgress")
end)

scenario("spy off", { spy = false }, function(H)
    install(H)
    H.load("GameProgress")
    H.check("spy off: reads emit no x_save_call", H.count("x_save_call") == 0)
    -- writes are always reported (the gate attributes career-file mtime changes to them)
    H.save("GameProgress")
    H.check("spy off: a write still emits x_save_call{fn,slot,active}", H.count("x_save_call", function(f)
        return f.fn == "SaveGameToSlot" and f.slot == "GameProgress" and f.active ~= nil
    end) == 1)
end)

scenario("selftest", nil, function(H)
    local SG = install(H)
    local seen
    local gs = { DoesSaveGameExist = function(self, slot, idx)
        local ps = H.Param(H.FStr(slot))
        H.hooks[H.GS .. "DoesSaveGameExist"]({}, ps, H.Param(idx))
        seen = ps.v.s
        return false
    end }
    local ok, detail = SG.selftest(gs)
    H.check("probe rewritten through the hook", ok == true and seen == "HSMP_3_probe", tostring(detail) .. " " .. tostring(seen))
    H.check("probe emits no save events", H.count("save_redirected") == 0 and H.count("x_save_call") == 0)
    local ok2, detail2 = SG.selftest({ DoesSaveGameExist = function() return false end })
    H.check("detects a hook that never fires", ok2 == false and tostring(detail2):find("did not fire") ~= nil, detail2)
end)

scenario("seed_session_slot", nil, function(H)
    local SG = install(H)
    local saved = 0
    local gi = { IsValid = function() return true end,
                 ["Save Game"] = function(self)
                     saved = saved + 1
                     -- GI "Save Game": DoesSaveGameExist + LoadGameFromSlot + SaveGameToSlot("GameProgress")
                     H.exists("GameProgress"); H.load("GameProgress")
                     H.last_seed_slot = (H.save("GameProgress"))
                 end }
    local ok, detail = SG.seed_session_slot(gi)
    H.check("refuses while inactive (career would be written)", ok == false and saved == 0, detail)
    SG.set_active(true)
    ok, detail = SG.seed_session_slot(gi)
    H.check("active: GI Save Game diverted into the session slot", ok == true and H.last_seed_slot == "HSMP_3_GameProgress",
        tostring(detail) .. " " .. tostring(H.last_seed_slot))
    H.check("after seeding, arena Load Game reads the session slot", H.load("GameProgress") == "HSMP_3_GameProgress")
    local ok2, d2 = SG.seed_session_slot({ IsValid = function() return true end, ["Save Game"] = function() end })
    H.check("detects a Save Game that wrote nothing", ok2 == false, d2)
end)

scenario("BP name hygiene", nil, function(H)
    -- Half Sword BP UFunctions have spaces; CamelCase calls silently no-op.
    local src = H.sg_src
    H.check("GI reload uses the real BP name", src:find('gi["Load Game"](gi)', 1, true) ~= nil)
    H.check("no CamelCase LoadGame(", src:find("LoadGame(", 1, true) == nil)
    H.check("seed uses the real BP name", src:find('gi["Save Game"](gi)', 1, true) ~= nil)
    H.check("no CamelCase SaveGame(", src:find(":SaveGame(", 1, true) == nil)
end)

-- --- entry points --------------------------------------------------------------------

function SPEC.count() return #SPEC end

function SPEC.run(i, sg_src, state_dir, save_dir)
    local sc = SPEC[i]
    local H = new_harness(sg_src, state_dir, save_dir, sc.extra)
    H.sg_src = sg_src
    local ok, err = pcall(sc.fn, H)
    if not ok then H.check("scenario raised no error", false, err) end
    return sc.name, H.results
end

return SPEC
