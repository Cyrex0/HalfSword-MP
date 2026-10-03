-- HSMPMenu / autotest_mover.lua -- harness-only scripted mover (HSMP_AUTOTEST).
--
-- The gate's pose check judges stand-in smoothness, foot slide and arm/blade
-- tracking under motion, which needs a moving pawn: a fighter standing still
-- for the whole Live window makes jitter_ratio "not applicable". The harness
-- sends {"cmd":"move","arg":"<seconds>"} on .autotest.cmd.jsonl at Live; this
-- drives the LOCAL pawn through walk + arm-swing phases for that long, then
-- stands still and hands its input back.
--
-- How Willie_BP is driven (bytecode, pp_Willie_BP.txt; verified in game):
--   * walking: AddMovementInput alone does nothing because the BP sets
--     CharacterMovement.MaxWalkSpeed from |"Movement Input Vector"|, and the
--     move axis events (InpAxisEvt_Move ...) reset that vector to 0 every
--     frame while no key is pressed. So the pawn's own input is disabled for
--     the run and every frame the mover calls the BP's own axis events with
--     the phase's values, writes the vector and adds forced movement input.
--   * swings: velocity-change impulses on the right hand and forearm.
--     Guarding to swing (the right-guard axis, or R_Guarding/Any_Guarding
--     written directly, with "Aim Vector_0" circling) drops the polearm
--     0.2-0.4 s after the mover starts: the hand opens while guarding without
--     the real input, and a mid-round drop is not what the pose check measures.
--
-- Threading: the loop runs through the mod's LoopAsync shim (game thread).
-- Nothing is cached across frames except the pawn's FName (identity check);
-- the controller and pawn are re-fetched every frame, so a level change or
-- a possession swap ends the run without touching a stale object.

local M = {}

local AX_FWD = "InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14"
local AX_RIGHT = "InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19"
local STILL_S = 3.0      -- the last seconds stand still: an idle window for idle_rms

-- Phase plan (seconds): walk forward / strafe / back / strafe (2 s each) with
-- an arm swing eased in over the first second; the last STILL_S stand still.
-- Returns fwd, right, swing (0..1), sx, sy (swing direction) for time t.
function M.phase(t, secs)
    local walk_end = math.max(0, secs - STILL_S)
    if t >= walk_end then return 0, 0, 0, 0, 0 end
    -- Back first, forward third: a slot faces the arena's middle, and Pit's
    -- middle is a hole. Forward-first can carry a fighter 4 m past its slot
    -- into it (a real fall death that ends the round before the harness
    -- kill). This way the square never goes past the slot.
    local k = math.floor(t / 2) % 4
    local fwd = ({ -1, 0, 1, 0 })[k + 1]
    local right = ({ 0, 1, 0, -1 })[k + 1]
    local a = t * 2 * math.pi * 1.0
    return fwd, right, math.min(1, t), math.cos(a), math.sin(a)
end

-- ctx: { log = fn(fmt, ...), UEH = UEHelpers, loop = LoopAsync, now = os.clock }
function M.new(ctx)
    local self = { active = false, runs = 0 }
    local now = ctx.now or os.clock
    local FH, FL

    local function pawn_now()
        local pc, p
        pcall(function() pc = ctx.UEH.GetPlayerController() end)
        if pc and pc:IsValid() then pcall(function() p = pc.Pawn end) end
        if p and p:IsValid() then return pc, p end
        return pc, nil
    end
    local function fname(o) local n; pcall(function() n = o:GetFName():ToString() end); return n end

    -- arg: "<seconds>[,noswing][,nowalk]"
    function self.start(arg)
        arg = tostring(arg or "")
        local secs = math.max(1, math.min(tonumber(arg:match("^%s*(%d+)")) or 10, 60))
        local noswing, nowalk = arg:find("noswing") ~= nil, arg:find("nowalk") ~= nil
        if self.active then ctx.log("mover: already running - ignored"); return false end
        local pc, me = pawn_now()
        if not me then ctx.log("mover: no possessed pawn - nothing to move"); return false end
        FH = FH or FName("hand_r")
        FL = FL or FName("lowerarm_r")
        local name = fname(me)
        pcall(function() me:DisableInput(pc) end)
        pcall(function() pc:ResetIgnoreMoveInput() end)
        -- Self-cut guard: the swing impulses fling the pawn's OWN weapon, and
        -- with real head meshes on the modular weapons a flung glaive can cut
        -- its own wielder to death mid-run with no foe nearby and zero hit
        -- claims. Harness motion must not kill: "Get Damage" is skipped while the run lasts
        -- (Willie_BP returns early on "Invulnerable"); a server-declared death
        -- calls Death() directly and the harness kill comes after the run.
        local guard = false
        pcall(function()
            if me["Invulnerable"] ~= true then me["Invulnerable"] = true; guard = true end
        end)
        self.active, self.runs = true, self.runs + 1
        local t0, frames, moved = now(), 0, 0
        local l0; pcall(function() l0 = me:K2_GetActorLocation() end)
        ctx.log("mover: %s for %.0f s (%s%s, then %.0f s still)", tostring(name), secs,
            nowalk and "no walk" or "walk", noswing and ", no swing" or " + arm swing", STILL_S)
        local function finish(why)
            local pc2, p = pawn_now()
            if p and fname(p) == name then
                if guard then pcall(function() p["Invulnerable"] = false end) end
                pcall(function() p["Movement Input Vector"] = { X = 0, Y = 0, Z = 0 } end)
                pcall(function() p:EnableInput(pc2) end)
                local l; pcall(function() l = p:K2_GetActorLocation() end)
                if l and l0 then moved = math.sqrt((l.X - l0.X) ^ 2 + (l.Y - l0.Y) ^ 2) end
            elseif guard then
                -- Possession changed mid-run: Willies are pooled, so a guard
                -- left on would make a reused body unkillable. Look it up
                -- fresh by name (no object cached across frames).
                pcall(function()
                    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
                        if w:IsValid() and fname(w) == name then w["Invulnerable"] = false end
                    end
                end)
            end
            self.active = false
            ctx.log("mover: done (%s) after %d frames, pawn moved %.0f cm%s", why, frames, moved,
                guard and " (self-cut guard released)" or "")
        end
        ctx.loop(16, function()
            local stop
            local ok, err = pcall(function()
                local t = now() - t0
                local pc2, p = pawn_now()
                if not p or fname(p) ~= name then stop = "pawn changed / gone"; return end
                if t >= secs then stop = "time"; return end
                local fwd, right, swing, sx, sy = M.phase(t, secs)
                if nowalk then fwd, right = 0, 0 end
                local yaw = 0
                pcall(function() yaw = math.rad(pc2:GetControlRotation().Yaw) end)
                -- The square is laid out in WORLD directions from the arena's
                -- middle (the origin): first leg outward, never toward the
                -- middle first. The control yaw is not the slot's facing, so a
                -- control-relative square can walk fighters into Pit's hole in
                -- either order.
                local ox, oy = math.cos(yaw), math.sin(yaw)
                if l0 and (l0.X * l0.X + l0.Y * l0.Y) > 100 then
                    local n = math.sqrt(l0.X * l0.X + l0.Y * l0.Y)
                    ox, oy = l0.X / n, l0.Y / n
                end
                -- phase "fwd" = away from the middle (fwd is -1 on the first leg)
                local wx = -fwd * ox - right * oy
                local wy = -fwd * oy + right * ox
                local cf = wx * math.cos(yaw) + wy * math.sin(yaw)
                local cr = -wx * math.sin(yaw) + wy * math.cos(yaw)
                pcall(function() p[AX_FWD](p, cf) end)
                pcall(function() p[AX_RIGHT](p, cr) end)
                local dx, dy = wx, wy
                local len = math.sqrt(dx * dx + dy * dy)
                local v = len > 0 and { X = dx / len, Y = dy / len, Z = 0 } or { X = 0, Y = 0, Z = 0 }
                pcall(function() p["Movement Input Vector"] = v end)
                if len > 0 then pcall(function() p:AddMovementInput(v, 1.0, true) end) end
                if swing > 0 and not noswing then
                    local k = swing
                    pcall(function()
                        local m = p.Mesh
                        m:AddImpulse({ X = 160 * k * sx, Y = 160 * k * sy, Z = 100 * k * sy }, FH, true)
                        m:AddImpulse({ X = 70 * k * sx, Y = 70 * k * sy, Z = 40 * k * sy }, FL, true)
                    end)
                end
                frames = frames + 1
            end)
            if not ok then stop = "error: " .. tostring(err) end
            if stop then finish(stop); return true end
            return false
        end)
        return true
    end
    return self
end

return M
