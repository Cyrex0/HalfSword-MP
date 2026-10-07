-- HSMPParity / duel_driver.lua (dev only): walk my pawn to an opponent's stand-in and fight
-- it through Willie_BP's own input events, without OS input, teleports or impulses.
--
--   autotest parity "drive <seconds> [walk] [guard] [swing] [stop=<cm>] [amp=<axis>] [hz=<n>]"
--   autotest parity "drive off"
--
-- Every frame, while the run lasts, the pawn's own input is disabled (so the engine's 0.0
-- axis callbacks cannot alternate with ours and flip the guard's press/release gates) and:
--   * the PlayerController's control yaw is set toward the stand-in's pelvis: Willie_BP turns
--     its capsule toward "Current Control Rotation" at its own rate (tonus, consciousness);
--   * walk: the Move Forward axis event with 1 until within `stop` cm (default 130), then 0,
--     plus "Movement Input Vector" / AddMovementInput, as autotest_mover does (the BP reads
--     walk speed from that vector, which its move events reset every frame);
--   * guard: the Right Guard axis event with 1 (R_Guarding through the BP's own gates);
--   * swing: the Turn (mouse X) axis event with a square wave of +-amp (default 6) at `hz`
--     (default 1.2) and a small Look (mouse Y) wave: Update Aim moves "Aim Vector_0" by the
--     axis values while guarding, which is how the mouse swings the weapon.
-- Nothing is cached across frames but names; pawn, controller and stand-in are looked up
-- fresh every frame, so a level change or respawn ends the run untouched.

local M = {}

local AX_FWD = "InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14"
local AX_RIGHT = "InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19"
local AX_TURN = "InpAxisEvt_Turn Right / Left Mouse_K2Node_InputAxisEvent_16"
local AX_LOOK = "InpAxisEvt_Look Up / Down Mouse_K2Node_InputAxisEvent_17"
local AX_GUARD_R = "InpAxisEvt_Right Guard Axis_K2Node_InputAxisEvent_6"

-- ctx: { log, loop(ms, fn), now(), pawn() -> pc, pawn, standin() -> actor, peer, world() -> key }
function M.new(ctx)
    local self = { run = nil }

    local function call(o, name, v) return pcall(function() o[name](o, v) end) end
    local function fname(o) local n; pcall(function() n = o:GetFName():ToString() end); return n end

    function M.parse(arg)
        local o = { secs = 10, walk = false, guard = false, swing = false, stop = 130, amp = 6, hz = 1.2 }
        for w in tostring(arg or ""):gmatch("%S+") do
            local k, v = w:match("^(%a+)=([%d%.%-]+)$")
            if k and o[k] ~= nil then o[k] = tonumber(v)
            elseif tonumber(w) then o.secs = tonumber(w)
            elseif w == "walk" or w == "guard" or w == "swing" or w == "off" then o[w] = true
            else return nil, "unknown word " .. w end
        end
        if o.swing then o.guard = true end
        o.secs = math.max(1, math.min(o.secs, 600))
        return o
    end

    local function finish(s, why)
        local pc, p = ctx.pawn()
        if p and fname(p) == s.name then
            if s.o.guard then call(p, AX_GUARD_R, 0.0) end
            call(p, AX_FWD, 0.0); call(p, AX_RIGHT, 0.0)
            pcall(function() p["Movement Input Vector"] = { X = 0, Y = 0, Z = 0 } end)
            pcall(function() p:EnableInput(pc) end)
        end
        self.run = nil
        ctx.log("drive: done (%s) after %d frames; walked %.0f cm, closest %.0f cm, swings %d",
            why, s.frames, s.walked, s.closest or -1, s.swings)
    end

    function self.start(arg)
        local o, err = M.parse(arg)
        if not o then ctx.log("drive: %s", err); return end
        if o.off then
            if self.run then self.run.stop = "off" else ctx.log("drive: not running") end
            return
        end
        if self.run then ctx.log("drive: already running; 'drive off' first"); return end
        local pc, me = ctx.pawn()
        if not me then ctx.log("drive: no possessed pawn"); return end
        local si, peer = ctx.standin()
        if not si then ctx.log("drive: no stand-in to fight"); return end
        local s = { o = o, name = fname(me), world = ctx.world(), t0 = ctx.now(), frames = 0, walked = 0, swings = 0, sign = 0 }
        pcall(function() local l = me:K2_GetActorLocation(); s.last = { l.X, l.Y } end)
        pcall(function() me:DisableInput(pc) end)
        self.run = s
        ctx.log("drive: %s -> stand-in of peer %s for %.0f s (%s%s%s, stop %.0f cm, amp %.1f, %.1f Hz)", s.name, tostring(peer),
            o.secs, o.walk and "walk " or "", o.guard and "guard " or "", o.swing and "swing" or "", o.stop, o.amp, o.hz)
        ctx.loop(16, function()
            if self.run ~= s then return true end
            local stop = s.stop
            local ok, e = pcall(function()
                local t = ctx.now() - s.t0
                if ctx.world() ~= s.world then stop = "world changed"; return end
                if t >= o.secs then stop = stop or "time"; return end
                local pc2, p = ctx.pawn()
                if not p or fname(p) ~= s.name then stop = "pawn changed"; return end
                local si2 = ctx.standin()
                if not si2 then stop = "stand-in gone"; return end
                local a, b
                pcall(function() local l = p:K2_GetActorLocation(); a = { l.X, l.Y, l.Z } end)
                pcall(function() local l = si2:K2_GetActorLocation(); b = { l.X, l.Y, l.Z } end)
                if not (a and b) then stop = "no location"; return end
                local dx, dy = b[1] - a[1], b[2] - a[2]
                local d = math.sqrt(dx * dx + dy * dy)
                s.closest = math.min(s.closest or d, d)
                if s.last then s.walked = s.walked + math.sqrt((a[1] - s.last[1]) ^ 2 + (a[2] - s.last[2]) ^ 2) end
                s.last = { a[1], a[2] }
                if d > 1 then
                    local yaw = math.deg(math.atan(dy, dx))
                    pcall(function()
                        local r = pc2:GetControlRotation()
                        pc2:SetControlRotation({ Pitch = r.Pitch, Yaw = yaw, Roll = 0 })
                    end)
                end
                local fwd = (o.walk and d > o.stop) and 1.0 or 0.0
                call(p, AX_FWD, fwd); call(p, AX_RIGHT, 0.0)
                if fwd > 0 then
                    local v = { X = dx / d, Y = dy / d, Z = 0 }
                    pcall(function() p["Movement Input Vector"] = v end)
                    pcall(function() p:AddMovementInput(v, 1.0, true) end)
                else
                    pcall(function() p["Movement Input Vector"] = { X = 0, Y = 0, Z = 0 } end)
                end
                if o.guard then call(p, AX_GUARD_R, 1.0) end
                if o.swing and d <= o.stop + 60 then
                    local ph = (t * o.hz) % 1
                    local sx = ph < 0.5 and 1 or -1
                    if sx ~= s.sign then s.swings = s.swings + 1; s.sign = sx end
                    call(p, AX_TURN, sx * o.amp)
                    call(p, AX_LOOK, 0.3 * o.amp * math.sin(2 * math.pi * o.hz * t))
                else
                    call(p, AX_TURN, 0.0); call(p, AX_LOOK, 0.0)
                end
                s.frames = s.frames + 1
            end)
            if not ok then stop = "error: " .. tostring(e) end
            if stop then finish(s, stop); return true end
            return false
        end)
    end

    return self
end

return M
