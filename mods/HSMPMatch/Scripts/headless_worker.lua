-- One native authority world. This entry bypasses all client-only mods and
-- starts without a menu, viewport, sidecar or client session/spawn pipeline.
local M = {}
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
    local Role, HW, IPC, SG, D, Control, Prepare, PoseConfig, Boundary = load_module("hsmp_runtime_role"), load_module("hsmp_wg"),
        load_module("hsmp_ipc"), load_module("hsmp_saveguard"), load_module("director"), load_module("headless_control"), load_module("headless_prepare"), load_module("hsmp_pose_config"), load_module("headless_sample_boundary")
    local function log(format, ...) print(string.format("[HSMPNativeWorker] " .. format .. "\n", ...)) end
    if not Role or not Role.worker() or not HW or not IPC or not SG or not D or not Control or not Prepare or not PoseConfig or not Boundary
        or not LoopInGameThreadWithDelay then log("startup refused: worker dependencies unavailable"); return end
    local UEH = require("UEHelpers")
    local state_dir = (os.getenv("HSMP_STATE_DIR") or "hsmp_state"):gsub("\\", "/")
    local identity_dir = os.getenv("HSMP_NATIVE_IDENTITY_DIR") or state_dir
    local stop_file = os.getenv("HSMP_NATIVE_STOP_FILE")
    local arena = os.getenv("HSMP_NATIVE_ARENA") or "Map_Arena_Yard"
    local bind = os.getenv("HSMP_NATIVE_BIND") or "127.0.0.1:7777"
    local boot_only = os.getenv("HSMP_NATIVE_BOOT_ONLY") == "1"
    IPC.init({ mod = "HSMPMatch", state_dir = state_dir, log = log })
    local N = IPC.N
    if not N or not N.worker_input then log("startup refused: native input binding unavailable"); return end
    local HL = load_module("hsmp_log")
    if HL then HL.init({ mod = "HSMPMatch", state_dir = state_dir }) end
    SG.install({ mod = "HSMPMatch", state_dir = state_dir, log = HL, fresh_per_session = false, reload_gi_on_exit = false })
    SG.set_active(true)
    local WG = HW.new({ log = log, UEHelpers = UEH })
    local RVPModule = load_module("hsmp_rvp")
    local RVP = RVPModule and RVPModule.new({ log = log, ev = function(name, fields) if HL then HL.event(name, fields) end end })
    if RVP then pcall(function() RegisterLoadMapPreHook(function() RVP.on_loadmap() end) end) end
    local stopped, hosted, last_key, last_state, last_report, last_stop_poll = false, false, nil, nil, -1e9, -1e9
    local bootstrap, controller
    local sample_configured, frame_seq, sample_at, first_ai_name = false, 0, -1e9, nil
    local entity_bindings = {}
    local metrics = { sample_ok=0, sample_refused=0, dispatch=0, active_dispatch=0, input_refused=0, sample_min_ms=nil, sample_max_ms=0, last_input_error=nil, last_sample_error=nil }
    local function same_world(world, actor)
        if stopped or not WG.check() or not WG.settled() then return false end
        local token = WG.token()
        local current = WG.world()
        if not current or not current:IsValid() or not world or not world:IsValid()
            or current:GetAddress() ~= world:GetAddress() or current:GetFullName() ~= world:GetFullName() then return false end
        local own = actor:GetWorld()
        if not WG.same(token) then return false end
        return own and own:IsValid() and own:GetAddress() == current:GetAddress() and own:GetFullName() == current:GetFullName()
    end
    local function player(index)
        if stopped or not WG.check() or not WG.settled() then return nil end
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
        return pc, pawn, world
    end
    local function resolve(index)
        if type(index) == "string" then
            if stopped or not WG.check() or not WG.settled() then return nil end
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
        local pc, pawn, world = player(index)
        if not pc then return nil end
        local token = WG.token()
        local binding = { index = index, world_key = WG.key, pc_address = pc:GetAddress(), pc_name = pc:GetFName():ToString(),
            pawn_address = pawn:GetAddress(), pawn_name = pawn:GetFName():ToString(), pc = pc, pawn = pawn, world = world }
        if not WG.same(token) then return nil end
        local current_pc, current_pawn = player(index)
        if not current_pc or not WG.same(token) or current_pc:GetAddress() ~= binding.pc_address
            or current_pc:GetFName():ToString() ~= binding.pc_name or current_pawn:GetAddress() ~= binding.pawn_address
            or current_pawn:GetFName():ToString() ~= binding.pawn_name then return nil end
        binding.pc, binding.pawn = current_pc, current_pawn
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
    local function census()
        if stopped or not WG.check() or not WG.settled() then return nil, "world settling" end
        local pc0, pawn0, world = player(0)
        local pc1, pawn1 = player(1)
        if not pawn0 then return nil, "native PC0 has no Willie" end
        if not pawn1 then return nil, "native PC1 has no Willie" end
        if pawn0:GetAddress() == pawn1:GetAddress() or pc0:GetAddress() == pc1:GetAddress() then return nil, "native players share a binding" end
        if not preparation:prepare(0) or not preparation:prepare(1) then return nil, "native animation/input preparation refused" end
        pc0, pawn0, world = player(0)
        pc1, pawn1 = player(1)
        if not pawn0 or not pawn1 then return nil,"native players changed during preparation" end
        local ai, ai_count = nil, 0
        for _, pawn in pairs(FindAllOf("Willie_BP_C") or {}) do
            if not same_world(world, pawn0) then return nil, "world changed during census" end
            if pawn and pawn:IsValid() and same_world(world, pawn) and pawn:GetAddress() ~= pawn0:GetAddress()
                and pawn:GetAddress() ~= pawn1:GetAddress() and pawn.DED == false and pawn.Health > 0 then
                local owner = pawn.Controller
                if owner and owner:IsValid() and owner:GetClass():GetFName():ToString() == "AI_BP_C" then
                    ai_count = ai_count + 1
                    if not ai then ai = pawn:GetFName():ToString() end
                    -- Same animation qualification, retaining native AI input
                    -- and possession throughout (no DisableInput on an AI).
                    if not preparation:prepare(pawn:GetFName():ToString()) then return nil,"native AI animation preparation refused" end
                end
            end
        end
        if ai_count ~= 1 then return nil, ai_count < 1 and "native AI missing" or "extra active native AI in three-entity bootstrap" end
        if not first_ai_name then first_ai_name = ai end
        return { players = 2, ai = ai_count, pawn0 = pawn0:GetFName():ToString(), pawn1 = pawn1:GetFName():ToString(), first_ai = ai }
    end
    local env = D.make_ue_env({ WG = WG, UEHelpers = UEH, log = log, SG = SG, RVP = RVP })
    env.native_settled = WG.settled
    env.native_players_ready = function() local rows, reason = census(); return rows ~= nil, reason end
    env.native_status = function(state, reason)
        last_state = state
        log("state=%s reason=%s", state, reason or "")
        if HL then HL.event("x_native_worker", { state = state, reason = reason or "", arena = arena }) end
        if hosted and N.host_status then N.host_status(state, reason or "") end
    end
    controller = Control.new({ now_ms = function() return os.clock() * 1000 end,
        running = function() return not stopped and bootstrap and bootstrap.state == "native_ready" and WG.check() and WG.settled() end,
        binding = function(row)
            local pc, pawn, world = player(row.controller)
            if not pc or not preparation:prepare(row.controller) then return nil end
            return { key = WG.key .. ":" .. tostring(pc:GetAddress()) .. ":" .. tostring(pawn:GetAddress()) .. ":" .. pawn:GetFName():ToString(),
                world_key = WG.key, world = world, pc = pc, pawn = pawn, pawn_name = pawn:GetFName():ToString(),
                pc_address = pc:GetAddress(), pawn_address = pawn:GetAddress(), entity_id = row.id, incarnation = row.incarnation, controller_index = row.controller }
        end,
        same = function(binding)
            if stopped or binding.world_key ~= WG.key or not same_world(binding.world, binding.pawn) then return false end
            return binding.pawn:GetFName():ToString() == binding.pawn_name and binding.pawn:GetAddress() == binding.pawn_address
                and binding.pc:GetAddress() == binding.pc_address and binding.pc.Pawn:GetAddress() == binding.pawn_address
                and binding.pawn.Controller:GetAddress() == binding.pc_address
        end,
        invoke = function(binding, axes, changed, buttons)
            local accepted, why = N.worker_input({ pawn = binding.pawn_address, controller = binding.pc_address, axes = axes, changed = changed, buttons = buttons })
            if accepted == true then
                metrics.dispatch = metrics.dispatch + 1
                local active = changed ~= 0 or buttons ~= 0
                for _, axis in ipairs(axes) do if axis ~= 0 then active = true end end
                if active then metrics.active_dispatch = metrics.active_dispatch + 1 end
                metrics.last_dispatched_entity, metrics.last_dispatched_controller = binding.entity_id, binding.controller_index
            else
                metrics.input_refused = metrics.input_refused + 1
                if why ~= metrics.last_input_error then log("native input refused: entity=%s controller=%s reason=%s", tostring(binding.entity_id), tostring(binding.controller_index), tostring(why)); metrics.last_input_error = why end
            end
            return accepted == true
        end,
    })
    local function sample_world(directory)
        if not N.native_sample_world or not N.sample_config then return false, "canonical sampler unavailable" end
        if not sample_configured then
            local ready, why = N.sample_config(PoseConfig)
            if ready ~= true then return false, "sample config: " .. tostring(why) end
            sample_configured = true
        end
        local token, actors = WG.token(), {}
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
            actors[#actors+1] = actor
        end
        if not WG.same(token) then return false, "world changed before canonical sample" end
        frame_seq = frame_seq + 1
        local now_ms = os.clock()*1000
        local before = os.clock()*1000
        local ok, why = Boundary.sample({sample=N.native_sample_world,commit=N.native_commit_world,same=WG.same,
            invalidate=function()if hosted and N.host_world_changed then N.host_world_changed() end;IPC.world_leaving()end},token,
            {epoch=directory.epoch,dir_seq=directory.seq,frame_seq=frame_seq,ts_ms=now_ms,dt_ms=sample_at > 0 and now_ms-sample_at or 0,actors=actors})
        local elapsed = os.clock()*1000-before
        if ok == true then
            metrics.sample_min_ms = math.min(metrics.sample_min_ms or elapsed,elapsed)
            metrics.sample_max_ms = math.max(metrics.sample_max_ms,elapsed)
            sample_at = now_ms
        end
        return ok == true, why
    end
    bootstrap = D.new_native_worker(env, { arena = arena })
    WG.on_drop(function()
        preparation:drop()
        last_key = nil
        first_ai_name, entity_bindings = nil, {}
        sample_at = -1e9
        controller:drop()
        if hosted and N.host_world_changed then N.host_world_changed() end
        IPC.world_leaving()
    end)
    M.stop = function(reason, fault)
        if stopped then return end
        stopped = true -- stop admission before endpoint teardown or engine callbacks
        controller:stop()
        bootstrap.stopped = true
        if fault then env.native_status("error",fault)
        else env.native_status("stopped",reason or "worker stopped") end
        if N.host_stop then N.host_stop() end
        local ok, quit = pcall(env.quit_native_worker)
        if not ok or quit ~= true then log("graceful native quit unavailable: %s",tostring(quit)) end
    end
    log("starting native authority: arena=%s bind=%s boot_only=%s", arena, bind, tostring(boot_only))
    LoopInGameThreadWithDelay(16, function()
        if stopped then return true end
        local ok, err = pcall(function()
            if N.host_parent_alive and N.host_parent_alive() ~= true then M.stop("native supervisor exited");return end
            if stop_file and os.clock() - last_stop_poll >= 0.25 then
                last_stop_poll = os.clock()
                if IPC.read(stop_file) then M.stop("stop requested"); return end
            end
            if not hosted and not boot_only then
                if not N.host_start then error("embedded native endpoint unavailable") end
                local started, why = N.host_start(bind, identity_dir, arena)
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
                local directory = N.host_directory()
                if controller:set_directory(directory) then
                    controller:tick() -- identify the pawn before accepting its first input
                    for _, frame in ipairs(N.host_inputs(32) or {}) do controller:receive(frame) end
                    if os.clock()*1000-sample_at >= 33 then
                        local sampled, why = sample_world(directory)
                        if sampled then metrics.sample_ok=metrics.sample_ok+1
                        else
                            metrics.sample_refused=metrics.sample_refused+1
                            if why~=metrics.last_sample_error then log("canonical sample refused: %s",tostring(why));metrics.last_sample_error=why end
                            if why=="canonical incarnation changed" or why=="native incarnation changed" or why=="native Health unavailable" or why=="native Health invalid" then
                                if N.host_world_changed then N.host_world_changed() end
                                error(why)
                            end
                        end
                    end
                end
            end
            if os.clock() - last_report >= 1 then
                last_report = os.clock()
                local rows, why = census()
                if rows then
                    log("native census: players=%d ai=%d pc0=%s pc1=%s first_ai=%s world=%s", rows.players, rows.ai, rows.pawn0, rows.pawn1, rows.first_ai, WG.key)
                    if HL then HL.event("x_native_worker_census", rows) end
                else log("native census unavailable: %s", why or "unknown") end
                log("native evidence: sampled=%d refused=%d sample_ms_min=%.3f sample_ms_max=%.3f dispatched=%d active_dispatched=%d input_refused=%d last_entity=%s last_controller=%s",
                    metrics.sample_ok,metrics.sample_refused,metrics.sample_min_ms or 0,metrics.sample_max_ms,metrics.dispatch,metrics.active_dispatch,metrics.input_refused,
                    tostring(metrics.last_dispatched_entity or ""),tostring(metrics.last_dispatched_controller or ""))
            end
        end)
        if not ok then log("worker stopped: %s", tostring(err)); M.stop(nil,tostring(err)); return true end
        if stopped then return true end
        return false
    end)
end
return M
