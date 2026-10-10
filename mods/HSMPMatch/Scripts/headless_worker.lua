-- One native authority world. This entry bypasses all client-only mods and
-- starts without a menu, viewport, sidecar or client session/spawn pipeline.
local M = {}
local started = false
local function load_module(name)
    local ok, value = pcall(require, name)
    if ok and type(value) == "table" then return value end
    local source = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local directory = source:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ directory .. "/" .. name .. ".lua", directory .. "/../../shared/" .. name .. ".lua" }) do
        ok, value = pcall(dofile, path)
        if ok and type(value) == "table" then return value end
    end
end
function M.start()
    if started then return false, "already started" end
    started = true -- never reinstall hooks or listeners after startup/refusal
    local Role, HW, IPC, SG, D, Control, Prepare, PoseConfig, Boundary, SpawnDiagnostics = load_module("hsmp_runtime_role"), load_module("hsmp_wg"),
        load_module("hsmp_ipc"), load_module("hsmp_saveguard"), load_module("director"), load_module("headless_control"), load_module("headless_prepare"), load_module("hsmp_pose_config"), load_module("headless_sample_boundary"), load_module("headless_spawn_diagnostics")
    local function log(format, ...) print(string.format("[HSMPNativeWorker] " .. format .. "\n", ...)) end
    if not Role or not Role.worker() or not HW or not IPC or not SG or not D or not Control or not Prepare or not PoseConfig or not Boundary or not SpawnDiagnostics
        or not LoopInGameThreadWithDelay then log("startup refused: worker dependencies unavailable"); return end
    local UEH = require("UEHelpers")
    local state_dir = (os.getenv("HSMP_STATE_DIR") or "hsmp_state"):gsub("\\", "/")
    local identity_dir = os.getenv("HSMP_NATIVE_IDENTITY_DIR") or state_dir
    local stop_file = os.getenv("HSMP_NATIVE_STOP_FILE")
    local arena = os.getenv("HSMP_NATIVE_ARENA") or "Map_Arena_Yard"
    local bind = os.getenv("HSMP_NATIVE_BIND") or "127.0.0.1:7777"
    local boot_only = os.getenv("HSMP_NATIVE_BOOT_ONLY") == "1"
    local native_mode = os.getenv("HSMP_NATIVE_MODE") or "pvp"
    local gameplay = os.getenv("HSMP_NATIVE_GAMEPLAY") == "1"
    IPC.init({ mod = "HSMPMatch", state_dir = state_dir, log = log })
    local N = IPC.N
    if not N or not N.worker_input then log("startup refused: native input binding unavailable"); return end
    -- The first frame captures native game-thread admission before any host
    -- endpoint, parent watchdog, or reflected native helper can be called.
    IPC.frame()
    local HL = load_module("hsmp_log")
    if HL then HL.init({ mod = "HSMPMatch", state_dir = state_dir }) end
    SG.install({ mod = "HSMPMatch", state_dir = state_dir, log = HL, fresh_per_session = false, reload_gi_on_exit = false })
    SG.set_active(true)
    local WG = HW.new({ log = log, UEHelpers = UEH })
    local RVPModule = load_module("hsmp_rvp")
    local RVP = RVPModule and RVPModule.new({ log = log, ev = function(name, fields) if HL then HL.event(name, fields) end end })
    if RVP then pcall(function() RegisterLoadMapPreHook(function() RVP.on_loadmap() end) end) end
    local stopped, hosted, last_key, last_state, last_report = false, false, nil, nil, -1e9
    local codec_reported=false
    local cancel_reason, cancel_fault, last_cancel_poll = nil, nil, -1e9
    local bootstrap, controller, loop_handle, source_lifecycle
    local sample_configured, frame_seq, sample_attempt_at, first_ai_name = false, 0, -1e9, nil
    local entity_bindings = {}
    local metrics = { sample_ok=0, sample_refused=0, dispatch=0, active_dispatch=0, active_pc0=0, active_pc1=0, input_refused=0, sample_min_ms=nil, sample_max_ms=0, last_input_error=nil, last_sample_error=nil, roster_wait=nil }
    local function admitted()
        if stopped or cancel_reason then return false end
        local now = os.clock()
        if now - last_cancel_poll < 0.25 then return true end
        last_cancel_poll = now
        -- Only latch refusal here. A collector's pcall and native scope end
        -- must unwind before endpoint teardown or Director engine callbacks.
        local ok, reason, fault = pcall(function()
            if stop_file and IPC.read(stop_file) then return "stop requested" end
            if N.host_parent_alive then
                local alive, why = N.host_parent_alive()
                if alive == false then return "native supervisor exited" end
                if alive ~= true then
                    local message = "native supervisor check refused: " .. tostring(why)
                    return message, message
                end
            end
        end)
        if not ok then reason, fault = "native cancellation check refused: " .. tostring(reason), "native cancellation check refused: " .. tostring(reason) end
        if reason then cancel_reason, cancel_fault = reason, fault; return false end
        return true
    end
    local phase_seen, phase_count, phase_limited, phase_cycle = {}, 0, false, 0
    local function source_phase(context, stage, edge, detail)
        if stopped then return end
        if not admitted() then error(cancel_reason,0) end
        if not HL or stopped or type(stage)~="string" or #stage>64 or (edge~="enter" and edge~="exit") then return end
        context,detail=type(context)=="table" and context or {},type(detail)=="table" and detail or {}
        local event={state="native_capture_phase",reason="",arena=arena,stage=stage,edge=edge}
        local epoch=rawget(context,"epoch")
        -- The shared JSON number encoder uses floating formatting. Preserve
        -- the original Lua integer separately; a float cannot supply its bits.
        event.epoch_text=type(epoch)=="number" and math.type(epoch)=="integer" and tostring(epoch) or "unknown"
        for _,key in ipairs({"epoch","id","incarnation","dir_seq","revision","frame_seq"})do
            local value=rawget(context,key)
            if type(value)=="number" and value==value and value~=math.huge and value~=-math.huge then event[key]=value end
        end
        for _,key in ipairs({"component_id","address","count","owner_id","pass","lod","mesh_census_calls","component_reads","parent_hops","qualifications"})do
            local value=rawget(detail,key)
            if type(value)=="number" and value==value and value~=math.huge and value~=-math.huge then event[key]=value end
        end
        for _,key in ipairs({"reason","name","class","getter"})do
            local value=rawget(detail,key)
            if type(value)=="string" then event[key]=value:sub(1,512) end
        end
        local accepted=rawget(detail,"ok")
        if type(accepted)=="boolean"then event.ok=accepted end
        -- Frame sequence is evidence, never a trace key. The bone loop can
        -- execute every frame while each exact generation/stage emits once.
        -- Only already-copied scalars may be stringified: no UObject methods.
        local parts={stage,edge}
        for _,key in ipairs({"epoch","id","incarnation","dir_seq","revision","component_id","address","name","pass","lod","owner_id"})do
            parts[#parts+1]=tostring(event[key] or "")
        end
        if stage=="canonical_core" or stage=="native_render" or stage=="canonical_publish"then parts[#parts+1]=tostring(phase_cycle)end
        local identity=table.concat(parts,"|")
        if phase_seen[identity]then return end
        if phase_count>=4096 then
            if not phase_limited then
                phase_limited=true
                HL.event("x_native_worker",{state="native_capture_phase",reason="capture trace record limit reached",arena=arena,stage="trace_limit",edge="exit"})
            end
            return
        end
        phase_seen[identity],phase_count=true,phase_count+1
        -- hsmp_log closes the append handle before returning: the entry is
        -- persisted before the following reflected/native operation starts.
        HL.event("x_native_worker",event)
    end
    local function same_world(world, actor)
        if not admitted() or not WG.check() or not WG.settled() then return false end
        local token = WG.token()
        local current = WG.world()
        if not current or not current:IsValid() or not world or not world:IsValid()
            or current:GetAddress() ~= world:GetAddress() or current:GetFullName() ~= world:GetFullName() then return false end
        local own = actor:GetWorld()
        if not WG.same(token) then return false end
        return own and own:IsValid() and own:GetAddress() == current:GetAddress() and own:GetFullName() == current:GetFullName()
    end
    local function player(index)
        if not admitted() or not WG.check() or not WG.settled() then return nil end
        local world = WG.world()
        local gs = UEH.GetGameplayStatics()
        if not world or not world:IsValid() or not gs or not gs:IsValid() then return nil end
        local token = WG.token()
        local pc = gs:GetPlayerController(world, index)
        if not pc or not pc:IsValid() or not WG.same(token) then return nil end
        local pc_address, pc_name = pc:GetAddress(), pc:GetFName():ToString()
        if not same_world(world, pc) then return nil end
        -- GetWorld may reenter possession/travel hooks. Re-resolve the index
        -- before reading the earlier controller or pawn again.
        pc = gs:GetPlayerController(world, index)
        if not pc or not pc:IsValid() or not WG.same(token) or pc:GetAddress() ~= pc_address
            or pc:GetFName():ToString() ~= pc_name then return nil end
        local pawn = pc.Pawn
        if not pawn or not pawn:IsValid() then return nil end
        local pawn_address, pawn_name = pawn:GetAddress(), pawn:GetFName():ToString()
        if not same_world(world, pawn) then return nil end
        pc = gs:GetPlayerController(world, index)
        if not pc or not pc:IsValid() or not WG.same(token) or pc:GetAddress() ~= pc_address
            or pc:GetFName():ToString() ~= pc_name then return nil end
        pawn = pc.Pawn
        if not pawn or not pawn:IsValid() or pawn:GetAddress() ~= pawn_address or pawn:GetFName():ToString() ~= pawn_name then return nil end
        if pawn:GetClass():GetFName():ToString() ~= "Willie_BP_C"
            or not pawn.Controller or not pawn.Controller:IsValid() or pawn.Controller:GetAddress() ~= pc:GetAddress() then return nil end
        -- All callback-capable world checks have finished. These copied scalar
        -- identities belong to the freshly re-resolved controller and pawn;
        -- returning them needs no second qualification or engine call.
        return pc, pawn, world, { index = index, world_key = token.key, pc_address = pc_address, pc_name = pc_name,
            pawn_address = pawn_address, pawn_name = pawn_name, pc = pc, pawn = pawn, world = world }
    end
    local function resolve(index)
        if type(index) == "string" then
            if not admitted() or not WG.check() or not WG.settled() then return nil end
            local token, pawn = WG.token(), nil
            for _, candidate in pairs(FindAllOf("Willie_BP_C") or {}) do
                if candidate and candidate:IsValid() and candidate:GetFName():ToString() == index then pawn=candidate;break end
            end
            if not pawn then return nil end
            local address = pawn:GetAddress()
            if not same_world(WG.world(), pawn) then return nil end
            pawn = nil -- re-find after GetWorld before another old actor read
            for _, candidate in pairs(FindAllOf("Willie_BP_C") or {}) do
                if candidate and candidate:IsValid() and candidate:GetFName():ToString() == index and candidate:GetAddress() == address then pawn=candidate;break end
            end
            if not pawn or not WG.same(token) then return nil end
            local owner = pawn.Controller
            if not owner or not owner:IsValid() or owner:GetClass():GetFName():ToString()~="AI_BP_C" then return nil end
            return { index=index, world_key=WG.key, pc_address=owner:GetAddress(), pc_name=owner:GetFName():ToString(),
                pawn_address=address, pawn_name=index, pc=owner, pawn=pawn, world=WG.world() }
        end
        local _, _, _, binding = player(index)
        return binding
    end
    local function current(binding)
        local fresh = resolve(binding.index)
        return Prepare.binding_same(binding, fresh) and fresh or nil
    end
    local preparation = Prepare.new({ resolve = resolve,
        disable_input = function(binding) if type(binding.index)~="string" then binding.pawn:DisableInput(binding.pc) end; return true end,
        reset_move = function(binding) if type(binding.index)~="string" then binding.pc:ResetIgnoreMoveInput() end; return true end,
        reset_look = function(binding) if type(binding.index)~="string" then binding.pc:ResetIgnoreLookInput() end; return true end,
        mesh = function(binding, field)
            local fresh = current(binding)
            if not fresh then return nil end
            local mesh = fresh.pawn[field]
            fresh = current(binding)
            if not fresh or not mesh or not mesh:IsValid() then return nil end
            local address, name = mesh:GetAddress(), mesh:GetFName():ToString()
            fresh = current(binding)
            if not fresh then return nil end
            local latest = fresh.pawn[field]
            if not latest or not latest:IsValid() or latest:GetAddress() ~= address or latest:GetFName():ToString() ~= name then return nil end
            local owner = latest:GetOwner()
            fresh = current(binding)
            if not fresh or not owner or not owner:IsValid() or owner:GetAddress() ~= binding.pawn_address then return nil end
            latest = fresh.pawn[field]
            if not latest or not latest:IsValid() or latest:GetAddress() ~= address or latest:GetFName():ToString() ~= name then return nil end
            return { address = address, name = name, owner = binding.pawn_address, object = latest }
        end,
        always_tick = function(_, mesh) mesh.object.VisibilityBasedAnimTickOption = 0; return mesh.object.VisibilityBasedAnimTickOption == 0 end,
        no_rate_skip = function(_, mesh) mesh.object.bEnableUpdateRateOptimizations = false; return mesh.object.bEnableUpdateRateOptimizations == false end,
        tick_enabled = function(_, mesh) mesh.object:SetComponentTickEnabled(true); return true end,
    })
    local function census_player(index)
        local pc, pawn, world = player(index)
        if not pc or not pawn then return nil end
        -- Only scalar/native property reads after the final possession lookup.
        -- Never carry this pawn wrapper through the next reflected player call.
        local health, team = pawn.Health, pawn["Team Int"]
        return { pc = pc:GetAddress(), pawn = pawn:GetAddress(), name = pawn:GetFName():ToString(),
            world = world, health = health, dead = pawn.DED, team = team }
    end
    local function census()
        if stopped or not WG.check() or not WG.settled() then return nil, "world settling" end
        local token = WG.token()
        local p0, p1 = census_player(0), census_player(1)
        if not p0 then return nil, "native PC0 has no Willie" end
        if not p1 then return nil, "native PC1 has no Willie" end
        if not WG.same(token) then return nil, "world changed during player census" end
        if p0.pawn == p1.pawn or p0.pc == p1.pc then return nil, "native players share a binding" end
        if not preparation:prepare(0) or not preparation:prepare(1) then return nil, "native animation/input preparation refused" end
        local a0, a1 = census_player(0), census_player(1)
        if not a0 or not a1 or not WG.same(token) or a0.pawn ~= p0.pawn or a1.pawn ~= p1.pawn
            or a0.name ~= p0.name or a1.name ~= p1.name then return nil,"native players changed during preparation" end
        local ai, ai_count = nil, 0
        for _, candidate in pairs(FindAllOf("Willie_BP_C") or {}) do
            if not WG.same(token) then return nil, "world changed during census" end
            local pawn
            if candidate and candidate:IsValid() then
                local address, name = candidate:GetAddress(), candidate:GetFName():ToString()
                if same_world(a0.world, candidate) and WG.same(token) then
                    for _, fresh in pairs(FindAllOf("Willie_BP_C") or {}) do
                        if fresh and fresh:IsValid() and fresh:GetAddress() == address and fresh:GetFName():ToString() == name then pawn = fresh; break end
                    end
                end
            end
            if not WG.same(token) then return nil, "world changed during census" end
            if pawn and pawn:GetAddress() ~= a0.pawn
                and pawn:GetAddress() ~= a1.pawn and pawn.DED == false and pawn.Health > 0 then
                local owner = pawn.Controller
                if owner and owner:IsValid() and owner:GetClass():GetFName():ToString() == "AI_BP_C" then
                    ai_count = ai_count + 1
                    if not ai then ai = pawn:GetFName():ToString() end
                    -- Same animation qualification, retaining native AI input
                    -- and possession throughout (no DisableInput on an AI).
                    if not preparation:prepare(pawn:GetFName():ToString()) then return nil,"native AI animation preparation refused" end
                else return nil, "untracked living native fighter" end
            end
        end
        local fresh0, fresh1 = census_player(0), census_player(1)
        if not fresh0 or not fresh1 or not WG.same(token) or fresh0.pawn ~= a0.pawn or fresh1.pawn ~= a1.pawn
            or fresh0.name ~= a0.name or fresh1.name ~= a1.name then return nil, "native players changed during census" end
        for _, row in ipairs({fresh0, fresh1}) do
            if type(row.health) ~= "number" or row.health ~= row.health or row.health <= 0 or row.dead ~= false then return nil, "native player is not living" end
        end
        local expected_ai = native_mode == "diagnostic" and 1 or 0
        if ai_count ~= expected_ai then return nil, ai_count < expected_ai and "native diagnostic AI missing" or "extra active native AI in authority bootstrap" end
        local team0, team1 = fresh0.team, fresh1.team
        if type(team0) ~= "number" or team0 % 1 ~= 0 or team0 < -2147483648 or team0 > 2147483647 or
            type(team1) ~= "number" or team1 % 1 ~= 0 or team1 < -2147483648 or team1 > 2147483647 then return nil, "native player team unavailable" end
        if native_mode == "pvp" and team0 == team1 then return nil, "native PvP players share a native team" end
        if not first_ai_name then first_ai_name = ai end
        return { players = 2, ai = ai_count, pawn0 = fresh0.name, pawn1 = fresh1.name, first_ai = ai or "", team0 = team0, team1 = team1 }
    end
    local env = D.make_ue_env({ WG = WG, UEHelpers = UEH, log = log, SG = SG, RVP = RVP })
    -- Capture the failed native path before changing it. All actor reads below
    -- use a fresh world-qualified lookup after GetWorld can reenter engine code.
    local function diagnostic_actor(class, address, name, token)
        local actor
        for _, candidate in pairs(FindAllOf(class) or {}) do
            if not WG.same(token) then return nil end
            if candidate and candidate:IsValid() and (not address or candidate:GetAddress() == address)
                and (not name or candidate:GetFName():ToString() == name) then
                local a, n = candidate:GetAddress(), candidate:GetFName():ToString()
                if same_world(WG.world(), candidate) and WG.same(token) then
                    for _, fresh in pairs(FindAllOf(class) or {}) do
                        if fresh and fresh:IsValid() and fresh:GetAddress() == a and fresh:GetFName():ToString() == n then actor = fresh; break end
                    end
                    if actor then break end
                end
            end
        end
        return WG.same(token) and actor or nil
    end
    local function diagnostic_flags(class, fields, token)
        local actor = diagnostic_actor(class, nil, nil, token)
        if not actor then return { state = "missing" } end
        local result = { name = actor:GetFName():ToString(), address = actor:GetAddress() }
        for _, field in ipairs(fields) do
            local value = actor[field]
            result[field] = SpawnDiagnostics.property(value)
        end
        return result
    end
    env.native_spawn_diagnostics = function(reason)
        if stopped or not WG.check() or not WG.settled() then return end
        local token = WG.token()
        local readers = { same = WG.same, world_key = function() return WG.key end,
            profile = function()
                local result = {}
                for _, row in ipairs(D.native_worker_profile(native_mode) or {}) do
                    if not WG.same(token) then return nil, "world changed during GI profile" end
                    local value = env.gi_get(row[1])
                    result[row[1]] = SpawnDiagnostics.property(value)
                end
                return result
            end,
            controllers = function()
                local result = {}
                local world, gs = WG.world(), UEH.GetGameplayStatics()
                if not WG.same(token) or not world or not world:IsValid() or not gs or not gs:IsValid() then return nil, "controller lookup unavailable" end
                for index = 0, 1 do
                    local row = { pc = "missing", pawn = "missing" }
                    result[index + 1] = row
                    local pc = gs:GetPlayerController(world, index)
                    if not WG.same(token) then return nil end
                    if pc and pc:IsValid() then
                        local address, name = pc:GetAddress(), pc:GetFName():ToString()
                        if not same_world(world, pc) or not WG.same(token) then return nil end
                        pc = gs:GetPlayerController(world, index)
                        if not WG.same(token) then return nil end
                        if not pc or not pc:IsValid() or pc:GetAddress() ~= address or pc:GetFName():ToString() ~= name then return nil, "controller binding changed" end
                        row.pc, row.pc_address, row.pc_class = name, address, pc:GetClass():GetFName():ToString()
                        local pawn = pc.Pawn
                        if pawn and pawn:IsValid() then
                            local pawn_address, pawn_name = pawn:GetAddress(), pawn:GetFName():ToString()
                            if not same_world(world, pawn) or not WG.same(token) then return nil end
                            pc = gs:GetPlayerController(world, index)
                            if not WG.same(token) then return nil end
                            if not pc or not pc:IsValid() or pc:GetAddress() ~= address or pc:GetFName():ToString() ~= name then return nil, "controller binding changed" end
                            pawn = pc.Pawn
                            if not pawn or not pawn:IsValid() or pawn:GetAddress() ~= pawn_address or pawn:GetFName():ToString() ~= pawn_name then return nil, "pawn binding changed" end
                            row.pawn, row.pawn_address, row.pawn_class = pawn_name, pawn_address, pawn:GetClass():GetFName():ToString()
                            if row.pawn_class == "Willie_BP_C" then
                                local team = pawn["Team Int"]
                                row.team = type(team) == "number" and team % 1 == 0 and team or "unavailable"
                            end
                        end
                    end
                end
                return result
            end,
            mode = function(t) return diagnostic_flags("BP_HalfSwordGameMode_C", { "Local Multiplayer", "Enemy Count", "All Enemies Dead", "Player DED" }, t) end,
            level = function(t) return diagnostic_flags("BP_LevelManager_C", { "Player Spawned", "P2 Spawned", "Amount of Characters to Spawn" }, t) end,
            willies = function(t)
                local result = { total = 0, ai = 0, alive_ai = 0, unpossessed = 0 }
                for _, candidate in pairs(FindAllOf("Willie_BP_C") or {}) do
                    if not WG.same(t) then return nil end
                    if candidate and candidate:IsValid() then
                        local pawn = diagnostic_actor("Willie_BP_C", candidate:GetAddress(), candidate:GetFName():ToString(), t)
                        if pawn then
                            result.total = result.total + 1
                            local owner = pawn.Controller
                            if not owner or not owner:IsValid() then result.unpossessed = result.unpossessed + 1
                            elseif owner:GetClass():GetFName():ToString() == "AI_BP_C" then
                                result.ai = result.ai + 1
                                if type(pawn.Health) == "number" and pawn.Health > 0 and pawn.DED == false then result.alive_ai = result.alive_ai + 1 end
                            end
                        end
                    end
                end
                return result
            end,
            spawners = function(t)
                local game,combat,play,foes=env.gi_get("Current Game Mode Enum"),env.gi_get("Current Combat Mode"),env.gi_get("Current Play Mode"),env.gi_get("Free Mode Foes Amount")
                if not WG.same(t) then return nil,"world changed during spawner context" end
                for _,pair in ipairs({{"game",game},{"combat",combat},{"play",play},{"foes",foes}})do
                    local value=pair[2]
                    if type(value)~="number" or value%1~=0 then return {state="native spawner context unavailable"} end
                end
                local result={total=0,compatible=0,player=0,nonplayer=0,compatible_player=0,compatible_nonplayer=0,
                    game=game,combat=combat,play=play,combatants=foes+1,actors={}}
                for _,candidate in pairs(FindAllOf("BP_SpawnerPoint_Willies_C") or {}) do
                    if not WG.same(t) then return nil end
                    if candidate and candidate:IsValid() then
                        local actor=diagnostic_actor("BP_SpawnerPoint_Willies_C",candidate:GetAddress(),candidate:GetFName():ToString(),t)
                        if actor then
                            result.total=result.total+1
                            if result.total>64 then return {state="native spawner actor bound"} end
                            local row={name=actor:GetFName():ToString(),address=actor:GetAddress(),
                                player=SpawnDiagnostics.property(actor["Spawn Player"]),mercenary=SpawnDiagnostics.property(actor["Spawn Mercenary"]),
                                required=SpawnDiagnostics.property(actor["Required Combatants Amount"]),spawned=SpawnDiagnostics.property(actor["Spawned Amount"])}
                            local valid=function()return WG.same(t)end
                            row.game=SpawnDiagnostics.map_flag(actor["Works in these Game Modes"],game,valid)
                            row.combat=SpawnDiagnostics.map_flag(actor["Works in these Combat Modes"],combat,valid)
                            row.play=SpawnDiagnostics.map_flag(actor["Works in these Play Modes"],play,valid)
                            row.compatible=SpawnDiagnostics.compatible(row.game,row.combat,row.play,foes+1,row.required)
                            if type(row.player)~="boolean" or row.compatible==nil then return {state="native spawner properties unavailable"} end
                            if row.player then result.player=result.player+1 else result.nonplayer=result.nonplayer+1 end
                            if row.compatible then
                                result.compatible=result.compatible+1
                                if row.player then result.compatible_player=result.compatible_player+1 else result.compatible_nonplayer=result.compatible_nonplayer+1 end
                            end
                            result.actors[#result.actors+1]=row
                        end
                    end
                end
                return result
            end,
        }
        local rows, why = SpawnDiagnostics.capture(readers, token, reason)
        if rows then
            log("native spawn diagnostic: %s", SpawnDiagnostics.format(rows))
            if HL then HL.event("x_native_worker", {state="native_spawn_diagnostic",reason=reason or "",arena=arena,census=rows}) end
        else
            log("native spawn diagnostic refused: %s", why or "binding changed")
            if HL then HL.event("x_native_worker", {state="native_spawn_diagnostic",reason=why or "binding changed",arena=arena}) end
        end
    end
    env.native_settled = WG.settled
    env.native_players_ready = function() local rows, reason = census(); return rows ~= nil, reason end
    env.native_status = function(state, reason)
        last_state = state
        log("state=%s reason=%s", state, reason or "")
        if HL then HL.event("x_native_worker", { state = state, reason = reason or "", arena = arena }) end
        if hosted and N.host_status then
            source_phase({},"host_status_"..state,"enter")
            N.host_status(state, reason or "")
            source_phase({},"host_status_"..state,"exit")
        end
    end
    local control_bindings = {resolve=resolve,prepare=function(index)return preparation:prepare(index)end}
    controller = Control.new({ ordered=gameplay, now_ms = function() return os.clock() * 1000 end,
        running = function() return not stopped and bootstrap and bootstrap.state == "native_ready" and WG.check() and WG.settled() end,
        binding = function(row)
            return Control.fresh_binding(row,control_bindings)
        end,
        same = function(binding)
            return not stopped and Control.binding_matches(binding,control_bindings)
        end,
        invoke = function(binding, axes, changed, buttons, request)
            local args = {pawn=binding.pawn_address,controller=binding.pc_address,axes=axes,changed=changed,buttons=buttons}
            if request and request.flags~=1 then
                for _,field in ipairs({"epoch","id","incarnation","seq","delivery_seq"}) do args[field]=request[field] end
            end
            local accepted, why = N.worker_input(args)
            if accepted == true then
                metrics.dispatch = metrics.dispatch + 1
                local active = changed ~= 0 or buttons ~= 0
                for _, axis in ipairs(axes) do if axis ~= 0 then active = true end end
                if active then
                    metrics.active_dispatch = metrics.active_dispatch + 1
                    local key = binding.controller_index == 0 and "active_pc0" or "active_pc1"
                    metrics[key] = metrics[key] + 1
                end
                metrics.last_dispatched_entity, metrics.last_dispatched_controller = binding.entity_id, binding.controller_index
            else
                metrics.input_refused = metrics.input_refused + 1
                if why ~= metrics.last_input_error then log("native input refused: entity=%s controller=%s reason=%s", tostring(binding.entity_id), tostring(binding.controller_index), tostring(why)); metrics.last_input_error = why end
            end
            return accepted == true
        end,
    })
    local function sample_world(directory)
        local sampler
        if gameplay then sampler=N.native_gameplay_sample else sampler=N.native_sample_world end
        if not sampler or not N.sample_config then return false, "canonical sampler unavailable" end
        if not sample_configured then
            local context={epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq+1}
            source_phase(context,"sample_config","enter")
            local ready, why = N.sample_config(PoseConfig)
            source_phase(context,"sample_config","exit",{ok=ready==true,reason=tostring(why or "")})
            if ready ~= true then return false, "sample config: " .. tostring(why) end
            sample_configured = true
        end
        local token, actors = WG.token(), {}
        if native_mode ~= "diagnostic" then
            local describe
            if gameplay then describe=N.host_gameplay_describe else describe=N.host_describe end
            if gameplay and type(N.host_gameplay_describe)~="function"then return false,"native gameplay bootstrap API unavailable"end
            if type(describe)~="function"or(not gameplay and not N.native_capture_render)then return false,"native source/render APIs unavailable"end
            local required={"native_source_scope_begin","native_source_scope_end","native_source_roster_facts"}
            if not gameplay then
                for _,name in ipairs({"native_source_scope_keep","native_source_scope_resolve","native_source_scope_spline_profile","native_source_scope_vertex_state"})do required[#required+1]=name end
            end
            for _,name in ipairs(required)do
                if type(N[name])~="function"then return false,"native source identity API unavailable: "..name end
            end
            if not source_lifecycle then
                local Adapter, Lifecycle = load_module("native_source_adapter"), load_module("headless_source_lifecycle")
                if not Adapter or not Lifecycle then return false, "native source descriptor modules unavailable" end
                local adapter = Adapter.new({ resolve=resolve, WG=WG, phase=source_phase,gameplay=gameplay,
                    source_scope={begin=N.native_source_scope_begin,keep=N.native_source_scope_keep,
                        resolve=N.native_source_scope_resolve,finish=N.native_source_scope_end,profile=N.native_source_scope_spline_profile,
                        vertex_state=N.native_source_scope_vertex_state} })
                source_lifecycle = Lifecycle.new({ resolve=resolve,same=WG.same,now_ms=function()return os.clock()*1000 end,
                    index=function(row)return row.kind==0 and row.controller or first_ai_name end,
                    capture=adapter.capture,discard=gameplay and adapter.discard or nil,
                    describe=describe,phase=source_phase,
                    directory=N.host_directory,roster_facts=N.native_source_roster_facts,
                    addresses=function(binding)return {world=binding.world:GetAddress(),pawn=binding.pawn_address,controller=binding.pc_address}end,
                    invalidate=function()if hosted and N.host_world_changed then N.host_world_changed() end;IPC.world_leaving()end })
            end
            local described, acknowledged = source_lifecycle.ensure(directory,token,frame_seq+1)
            if described ~= true then return described, acknowledged end
            directory=acknowledged -- only the exact team-ACK snapshot may reach core/render publication
        end
        for _, row in ipairs(directory.entities or {}) do
            if not WG.same(token) then return false, "world changed during actor lookup" end
            local binding = resolve(row.kind==0 and row.controller or first_ai_name)
            if not binding then return false,"canonical actor unavailable" end
            local pawn, name, address = binding.pawn, binding.pawn_name, binding.pawn_address
            local mesh = pawn.Mesh
            if not mesh or not mesh:IsValid() or not WG.same(token) then return false, "canonical mesh unavailable" end
            local binding_key = tostring(address) .. ":" .. name .. ":" .. tostring(mesh:GetAddress()) .. ":" .. mesh:GetFName():ToString()
            local reference = tostring(row.epoch) .. ":" .. tostring(row.id) .. ":" .. tostring(row.incarnation)
            if entity_bindings[reference] and entity_bindings[reference] ~= binding_key then return false, "canonical incarnation changed" end
            entity_bindings[reference] = binding_key
            local actor = { epoch=row.epoch, id=row.id, incarnation=row.incarnation, pawn=address, mesh=mesh:GetAddress(),
                controller=binding.pc_address,controller_index=row.controller,pawn_name=name,dism=0 }
            do
                local seen, count = {}, 0
                local two_handed = pawn["R Two Handed Grip"] == true
                for _, hand in ipairs({ "R", "L" }) do
                    local weapon = pawn["Weapon " .. hand]
                    if weapon and weapon:IsValid() and not seen[weapon:GetAddress()] then
                        local class = weapon:GetClass():GetFName():ToString()
                        if not class:find("Fists", 1, true) then
                            seen[weapon:GetAddress()] = true; count = count + 1
                            actor["w"..count] = weapon:GetAddress()
                            actor["h"..count] = hand == "R" and (two_handed and 3 or 1) or 2
                            -- Geometry is available; canonical gear/recipe metadata
                            -- needs the separate native descriptor contract.
                            actor["t"..count] = 0
                        end
                    end
                end
            end
            actors[#actors+1] = actor
        end
        if not WG.same(token) then return false, "world changed before canonical sample" end
        frame_seq = frame_seq + 1
        local now_ms = os.clock()*1000
        local before = os.clock()*1000
        if gameplay then
            -- The compact sampler publishes native root/stat values and each
            -- fighter's full physical pose (bodies + held weapons) for clients
            -- to display. Recipes are bootstrap data.
            source_phase({epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq},"gameplay_sample","enter")
            local ok, why = sampler({epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq,
                ts_ms=now_ms,dt_ms=0,actors=actors})
            source_phase({epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq},"gameplay_sample","exit",{ok=ok==true,reason=tostring(why or "")})
            local elapsed=os.clock()*1000-before
            if ok==true then
                metrics.sample_min_ms=math.min(metrics.sample_min_ms or elapsed,elapsed)
                metrics.sample_max_ms=math.max(metrics.sample_max_ms,elapsed)
            end
            return ok==true,why
        end
        local ok, why = Boundary.sample({sample=N.native_sample_world,commit=N.native_commit_world,same=WG.same,
            phase=source_phase,
            render=native_mode~="diagnostic" and N.native_capture_render or nil,allow_core_only=native_mode=="diagnostic",
            refresh=function()phase_cycle=phase_cycle+1;if source_lifecycle then source_lifecycle.refresh() end end,
            invalidate=function()if hosted and N.host_world_changed then N.host_world_changed() end;IPC.world_leaving()end},token,
            -- Pose step describes the sender's native physics timestep. No
            -- qualified engine timestep is captured here; zero means unknown.
            -- Keep the actual capture timestamp independent of scheduling.
            {epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq,ts_ms=now_ms,dt_ms=0,actors=actors})
        local elapsed = os.clock()*1000-before
        if ok == true then
            metrics.sample_min_ms = math.min(metrics.sample_min_ms or elapsed,elapsed)
            metrics.sample_max_ms = math.max(metrics.sample_max_ms,elapsed)
        end
        return ok == true, why
    end
    bootstrap = D.new_native_worker(env, { arena = arena, mode = native_mode })
    WG.on_drop(function()
        preparation:drop()
        last_key = nil
        first_ai_name, entity_bindings = nil, {}
        phase_seen, phase_count, phase_limited, phase_cycle = {}, 0, false, 0
        if source_lifecycle then source_lifecycle.drop() end
        sample_attempt_at = -1e9
        controller:drop()
        if hosted and N.host_world_changed then N.host_world_changed() end
        IPC.world_leaving()
    end)
    M.stop = function(reason, fault)
        if stopped then return end
        stopped = true -- stop admission before endpoint teardown or engine callbacks
        if loop_handle and CancelDelayedAction then CancelDelayedAction(loop_handle) end
        controller:stop()
        bootstrap.stopped = true
        if fault then env.native_status("error",fault)
        else env.native_status("stopped",reason or "worker stopped") end
        if N.host_stop then N.host_stop() end
        local ok, quit = pcall(env.quit_native_worker)
        if not ok or quit ~= true then log("graceful native quit unavailable: %s",tostring(quit)) end
    end
    log("starting native authority: mode=%s arena=%s bind=%s boot_only=%s", native_mode, arena, bind, tostring(boot_only))
    loop_handle = LoopInGameThreadWithDelay(16, function()
        if stopped then return true end
        local ok, err = pcall(function()
            if not admitted() then return end
            if not hosted and not boot_only then
                if not N.host_start then error("embedded native endpoint unavailable") end
                local started, why = N.host_start(bind, identity_dir, arena, native_mode)
                if started ~= true then error("native endpoint refused: " .. tostring(why)) end
                hosted = true
                if last_state and N.host_status then N.host_status(last_state, "") end
            end
            SG.tick()
            if not WG.check() then return end
            if WG.key ~= last_key then last_key = WG.key; IPC.world_ready(WG.key); env.apply_cvars() end
            IPC.frame(WG.key)
            if not bootstrap:tick() then
                if bootstrap.state=="error" then M.stop(nil,bootstrap.reason or "native bootstrap failed") end
                return
            end
            if hosted and N.host_directory then
                source_phase({},"native_directory","enter")
                local directory = N.host_directory()
                source_phase({},"native_directory","exit",{ok=type(directory)=="table"})
                if controller:set_directory(directory) then
                    local context={epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq+1}
                    source_phase(context,"native_control","enter")
                    controller:tick() -- identify the pawn before accepting its first input
                    if cancel_reason then return end
                    for _, frame in ipairs(N.host_inputs(32) or {}) do controller:receive(frame) end
                    if gameplay then controller:tick() end -- execute all admitted edges in order before sampling ACKs
                    source_phase(context,"native_control","exit")
                    local attempt_at=os.clock()*1000
                    if attempt_at-sample_attempt_at >= 33 then
                        -- Failed captures and pending roster ACKs are attempts
                        -- too. Never schedule from the last successful publish.
                        sample_attempt_at=attempt_at
                        local sampled, why = sample_world(directory)
                        if cancel_reason then return end
                        if sampled then metrics.sample_ok=metrics.sample_ok+1;metrics.last_sample_error=nil;metrics.roster_wait=nil
                        elseif sampled==nil then
                            -- An in-flight native roster ACK is a transient wait;
                            -- it cannot publish a partial recipe or count as a capture fault.
                            if metrics.roster_wait~=why then log("canonical sample waiting: %s",tostring(why));metrics.roster_wait=why end
                        else
                            metrics.roster_wait=nil
                            metrics.sample_refused=metrics.sample_refused+1
                            -- A bounded descriptor retry keeps the actual capture
                            -- refusal available instead of replacing it with a wait.
                            if why~="source descriptor retry pending" or not metrics.last_sample_error then
                                if why~=metrics.last_sample_error then log("canonical sample refused: %s",tostring(why));metrics.last_sample_error=why end
                            end
                            if why=="canonical incarnation changed" or why=="native incarnation changed" or why=="native Health unavailable" or why=="native Health invalid"
                                or why=="previous gameplay execution incomplete" then
                                if N.host_world_changed then N.host_world_changed() end
                                error(why)
                            end
                        end
                    end
                end
            end
            if os.clock() - last_report >= 1 then
                last_report = os.clock()
                if gameplay and not codec_reported and type(N.native_gameplay_metrics)=="function" then
                    local codec=N.native_gameplay_metrics()
                    if type(codec)=="table" and (codec.encode_samples or 0)>=8 then
                        codec_reported=true
                        log("compact codec: samples=%d raw_bytes=%d wire_bytes=%d encode_us=%d",codec.encode_samples,codec.raw_bytes,codec.wire_bytes,codec.encode_us)
                        codec.state="native_compression";codec.reason="";codec.arena=arena
                        if HL then HL.event("x_native_worker",codec) end
                    end
                end
                local rows, why = census()
                if rows then
                    log("native census: players=%d ai=%d pc0=%s pc1=%s first_ai=%s world=%s", rows.players, rows.ai, rows.pawn0, rows.pawn1, rows.first_ai, WG.key)
                    if HL then HL.event("x_native_worker_census", rows) end
                else log("native census unavailable: %s", why or "unknown") end
                log("native evidence: sampled=%d refused=%d sample_ms_min=%.3f sample_ms_max=%.3f dispatched=%d active_dispatched=%d active_pc0=%d active_pc1=%d input_refused=%d last_entity=%s last_controller=%s",
                    metrics.sample_ok,metrics.sample_refused,metrics.sample_min_ms or 0,metrics.sample_max_ms,metrics.dispatch,metrics.active_dispatch,
                    metrics.active_pc0,metrics.active_pc1,metrics.input_refused,tostring(metrics.last_dispatched_entity or ""),tostring(metrics.last_dispatched_controller or ""))
                if HL then HL.event("x_native_worker", {state="native_evidence",reason=tostring(metrics.last_sample_error or ""),arena=arena,frame_seq=frame_seq,
                    sampled=metrics.sample_ok,refused=metrics.sample_refused,active_pc0=metrics.active_pc0,active_pc1=metrics.active_pc1,
                    dispatched=metrics.dispatch,input_refused=metrics.input_refused}) end
            end
        end)
        if cancel_reason then M.stop(cancel_reason,cancel_fault); return true end
        if not ok then log("worker stopped: %s", tostring(err)); M.stop(nil,tostring(err)); return true end
        if stopped then return true end
        return false
    end)
end
return M
