-- Spectator camera offline tests (HSMPMatch/Scripts/spectate_cam.lua).
--
--   hsmp-tools lua-test spectate_cam
--
-- A mock world: a world key, stand-in positions by name, arena data, and the
-- camera actors the module spawns (with a per-world liveness). Covered: follow
-- (spawn once, view once with the fade cleared, behind the target, smoothed,
-- looking at it), a target that vanishes falling back to the arena view after
-- the grace time, the arena view orbiting high over the centre (arena data,
-- else the players' bounds), a level change dropping and rebuilding the
-- camera without touching the old one, and off.

local C = dofile(T.path("mods/HSMPMatch/Scripts/spectate_cam.lua"))

local function new_world()
    local w = { key = "W1", targets = {}, arena = { centre = { x = 0, y = 0, z = 0 }, radius = 1000 },
                cams = {}, views = {}, fades = 0, logs = {}, players = {} }
    w.env = {
        world_key = function() return w.key end,
        spawn_camera = function(pos, rot)
            local c = { world = w.key, alive = true, pos = pos, rot = rot, moves = 0 }
            w.cams[#w.cams + 1] = c
            return c
        end,
        cam_ok = function(c)
            assert(c.world == w.key, "a camera of an old world was touched")
            return c.alive
        end,
        place = function(c, pos, rot)
            assert(c.world == w.key, "a camera of an old world was moved")
            c.pos, c.rot, c.moves = pos, rot, c.moves + 1
        end,
        view = function(c, blend) w.views[#w.views + 1] = { cam = c, blend = blend } end,
        clear_fade = function() w.fades = w.fades + 1 end,
        target_pos = function(name) return w.targets[name] end,
        arena = function() return w.arena end,
        players = function() return w.players end,
        log = function(fmt, ...) w.logs[#w.logs + 1] = string.format(fmt, ...) end,
    }
    w.cam = C.new(w.env)
    w.t = 0
    function w:run(secs)
        local mode
        for _ = 1, math.floor(secs * 60 + 0.5) do
            self.t = self.t + 1 / 60
            mode = self.cam:frame(self.t, 1 / 60)
        end
        return mode
    end
    return w
end

local function dist(a, b) return math.sqrt((a.x - b.x) ^ 2 + (a.y - b.y) ^ 2 + (a.z - b.z) ^ 2) end
local function hdist(a, b) return math.sqrt((a.x - b.x) ^ 2 + (a.y - b.y) ^ 2) end

T.log("== off: nothing spawned")
do
    local w = new_world()
    T.check(w:run(1) == "off", "off by default")
    T.check(#w.cams == 0 and #w.views == 0, "no camera while not spectating")
end

T.log("== follow")
do
    local w = new_world()
    w.targets.Willie_BP_C_3 = { x = 500, y = 0, z = 100 }
    w.cam:follow("Willie_BP_C_3")
    T.check(w:run(0.5) == "follow", "follow mode")
    T.check(#w.cams == 1, "one camera spawned", #w.cams)
    T.check(#w.views == 1 and w.views[1].cam == w.cams[1], "viewed once through our camera", #w.views)
    T.check(w.fades == 1, "the camera fade was cleared on entry", w.fades)
    local c, p = w.cams[1], w.targets.Willie_BP_C_3
    local hd = hdist(c.pos, p)
    T.check(math.abs(hd - C.FOLLOW_DIST) < 5, "behind the target at the follow distance", hd)
    T.check(c.pos.z > p.z + 100, "above the target", c.pos.z)
    T.check(c.pos.x > p.x, "first frame from outside the arena (centre at the origin)", c.pos.x)
    local yaw = math.deg(math.atan(p.y - c.pos.y, p.x - c.pos.x))
    T.check(math.abs(((c.rot.yaw - yaw + 180) % 360) - 180) < 1, "looks at the target", c.rot.yaw)
    T.check(c.rot.pitch < 0, "looks down at it", c.rot.pitch)
    -- the target moves: the camera follows smoothly (no snap), then catches up
    w.targets.Willie_BP_C_3 = { x = 500, y = 600, z = 100 }
    w:run(1 / 60)
    local step = dist(c.pos, { x = 500 + C.FOLLOW_DIST, y = 0, z = 100 + C.FOLLOW_UP })
    T.check(step < 60, "smoothed: one frame moves only part of the way", step)
    w:run(3)
    local hd2 = hdist(c.pos, w.targets.Willie_BP_C_3)
    T.check(math.abs(hd2 - C.FOLLOW_DIST) < 15, "caught up behind the moved target", hd2)
    T.check(#w.cams == 1 and #w.views == 1, "same camera, no re-view while following", #w.cams .. "/" .. #w.views)
    -- cycling to another target re-views once, same camera
    w.targets.Willie_BP_C_5 = { x = -800, y = 0, z = 0 }
    w.cam:follow("Willie_BP_C_5")
    w:run(4)
    T.check(#w.cams == 1 and #w.views == 2, "cycling keeps the camera, views again", #w.cams .. "/" .. #w.views)
    T.check(math.abs(hdist(c.pos, w.targets.Willie_BP_C_5) - C.FOLLOW_DIST) < 15, "now behind the new target")
end

T.log("== invalid target -> arena view")
do
    local w = new_world()
    w.targets.A = { x = 100, y = 100, z = 0 }
    w.cam:follow("A")
    w:run(0.5)
    w.targets.A = nil
    T.check(w:run(C.LOST_S * 0.5) == "follow", "a short gap holds the last view (stand-in swap)")
    local held = w.cams[1].pos
    T.check(held ~= nil, "camera held")
    T.check(w:run(C.LOST_S) == "sky", "gone past the grace time: arena view")
    local c = w.cams[1]
    T.check(c.pos.z >= 1000, "high over the arena", c.pos.z)
    local yaw = math.deg(math.atan(0 - c.pos.y, 0 - c.pos.x))
    T.check(math.abs(((c.rot.yaw - yaw + 180) % 360) - 180) < 1, "looks at the arena centre")
    local p1 = { x = c.pos.x, y = c.pos.y, z = c.pos.z }
    w:run(2)
    T.check(dist(p1, c.pos) > 10, "the arena view orbits slowly")
    T.check(math.abs(hdist(c.pos, { x = 0, y = 0 }) - 1000 * C.SKY_ORBIT_R) < 1, "on the orbit radius")
    -- following a target that never existed: arena view too
    local w2 = new_world()
    w2.cam:follow("nobody")
    T.check(w2:run(C.LOST_S + 0.2) == "sky", "unknown target -> arena view")
    T.check(#w2.cams == 1 and #w2.views == 1, "a camera and a view even then")
end

T.log("== arena view without arena data: the players' bounds")
do
    local w = new_world()
    w.arena = nil
    w.players = { { x = 1000, y = 0, z = 0 }, { x = 3000, y = 0, z = 0 } }
    w.cam:sky()
    T.check(w:run(0.1) == "sky", "sky")
    local c = w.cams[1]
    T.check(math.abs(hdist(c.pos, { x = 2000, y = 0 }) - C.SKY_ORBIT_R * 1300) < 2, "orbit around the players' centre", T.repr(c.pos))
    local w2 = new_world()
    w2.arena, w2.players = nil, {}
    w2.cam:sky()
    w2:run(0.5)
    T.check(#w2.cams == 0, "no frame at all: no camera (keeps the old view)")
end

T.log("== level change rebuilds the camera")
do
    local w = new_world()
    w.targets.A = { x = 0, y = 400, z = 0 }
    w.cam:follow("A")
    w:run(0.5)
    local old = w.cams[1]
    w.key = "W2"        -- the old world (and its camera) is gone: never touched again
    w:run(0.5)
    T.check(#w.cams == 2 and w.cams[2].world == "W2", "a new camera in the new world", #w.cams)
    T.check(old.moves == old.moves and w.views[#w.views].cam == w.cams[2], "viewing through the new one")
    T.check(T.any(w.logs, function(l) return l:find("world change") end), "logged the drop")
    -- the world guard drop (before the key changes)
    w.cam:drop("world guard")
    w:run(0.1)
    T.check(#w.cams == 3, "a guard drop rebuilds too", #w.cams)
    -- no world for a while (loading): nothing spawned, nothing touched
    w.key = nil
    w:run(1)
    T.check(#w.cams == 3, "no camera without a world")
    w.key = "W3"
    w:run(0.1)
    T.check(#w.cams == 4 and w.cams[4].world == "W3", "rebuilt once the world is there")
    -- a camera destroyed under us (same world)
    w.cams[4].alive = false
    w:run(0.1)
    T.check(#w.cams == 5, "a dead camera actor is replaced")
end

T.log("== off and back on")
do
    local w = new_world()
    w.targets.A = { x = 0, y = 0, z = 0 }
    w.cam:follow("A")
    w:run(0.2)
    w.cam:off()
    local moves = w.cams[1].moves
    w:run(1)
    T.check(w.cams[1].moves == moves, "off: the camera is left alone")
    w.cam:follow("A")
    w:run(0.2)
    T.check(#w.cams == 1 and #w.views == 2 and w.fades == 2, "on again: same camera, fade cleared and viewed again")
end

T.log("== the view is held against the death flow")
do
    local w = new_world()
    local n = 0
    w.env.enforce = function(c) assert(c == w.cams[1]); n = n + 1 end
    w.targets.A = { x = 0, y = 0, z = 0 }
    w.cam:follow("A")
    w:run(2)
    T.check(n >= 17 and n <= 21, "re-asserted about every 100 ms while spectating", n)
    w.cam:off()
    local k = n
    w:run(1)
    T.check(n == k, "never while not spectating", n - k)
end

T.log("== never inside a wall: pulled in front of geometry; a cut snaps")
do
    local w = new_world()
    w.targets.A = { x = 0, y = 0, z = 0 }
    -- a wall 100 uu from the target on every side
    w.env.trace = function(a, b)
        local d = math.sqrt((b.x - a.x) ^ 2 + (b.y - a.y) ^ 2 + (b.z - a.z) ^ 2)
        if d > 100 then return 100 / d end
        return nil
    end
    w.cam:follow("A")
    w:run(1)
    local c = w.cams[1]
    local look = { x = 0, y = 0, z = C.LOOK_UP }
    T.check(dist(c.pos, look) <= 100, "in front of the wall", dist(c.pos, look))
    T.check(dist(c.pos, look) >= 0.15 * 300, "never on top of the target", dist(c.pos, look))
    -- arena view, then back to a target far away: no flight through the town
    w.env.trace = nil
    w.cam:sky(); w:run(1)
    w.targets.B = { x = 5000, y = 0, z = 0 }
    w.cam:follow("B"); w:run(1 / 60)
    T.check(math.abs(hdist(c.pos, w.targets.B) - C.FOLLOW_DIST) < 5, "a cut snaps behind the new target (no smoothing)", hdist(c.pos, w.targets.B))
end
