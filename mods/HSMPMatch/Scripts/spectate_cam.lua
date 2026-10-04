-- spectate_cam.lua -- HSMPMatch's spectator camera (after death, while the
-- round plays on).
--
-- Never views through a stand-in itself: a stand-in Willie's own camera is
-- whichever the shared game settings activated (the first-person one sits
-- inside its head and helmet: a black screen). We own one CameraActor per
-- world and move it every frame:
--   follow  third person behind the watched player (pelvis), smoothed, the
--           arena centre in the background;
--   sky     a high, slowly orbiting view of the whole arena (from the arena
--           data, else the players' bounds): when nobody alive is left to
--           watch or the follow target is gone.
-- On entry the camera manager's fade is cleared (a death fade left at full
-- alpha also reads as black).
--
-- Pure logic over `env` (tests drive it with a mock world):
--   env.world_key()          current world key (nil = no world yet)
--   env.spawn_camera(pos, rot) -> cam | nil
--   env.cam_ok(cam)          the camera actor is still usable
--   env.place(cam, pos, rot) move it (pos {x,y,z}, rot {pitch,yaw})
--   env.view(cam, blend_s)   make it the view target
--   env.clear_fade()         stop any camera fade
--   env.enforce(cam)         (optional) re-assert the view every ENFORCE_S
--   env.trace(from, to)      (optional) fraction of from->to before world geometry, nil = clear
--   env.target_pos(name)     watched player's position {x,y,z} or nil (gone)
--   env.arena()              { centre = {x,y,z}, radius = r } or nil
--   env.players()            positions {x,y,z} of the living players (fallback bounds)
--   env.log(fmt, ...)
-- UObjects never leave env: this module keeps only the opaque `cam` handle,
-- and drops it (Lua reference only) whenever the world key changes.

local C = {}
C.__index = C

C.FOLLOW_DIST = 320      -- uu behind the target
C.FOLLOW_UP = 140        -- uu above the pelvis
C.LOOK_UP = 30           -- look slightly above the pelvis
C.FOLLOW_RATE = 5.0      -- 1/s position smoothing
C.SKY_MIN_R = 700
C.SKY_ORBIT_DPS = 4.0    -- deg/s
C.SKY_ORBIT_R = 0.45     -- orbit radius, share of the arena radius
C.MIN_PULL = 0.15         -- the camera never comes closer than this share
C.LOST_S = 1.0           -- a target missing this long -> sky
C.BLEND_S = 0.4
C.ENFORCE_S = 0.1     -- re-assert view target, no fade, no death screen

local function v(x, y, z) return { x = x, y = y, z = z } end
local function sub(a, b) return v(a.x - b.x, a.y - b.y, a.z - b.z) end
local function add(a, b) return v(a.x + b.x, a.y + b.y, a.z + b.z) end
local function mul(a, s) return v(a.x * s, a.y * s, a.z * s) end
local function hlen(a) return math.sqrt(a.x * a.x + a.y * a.y) end

-- Rotation (deg) that looks from `from` at `to`.
function C.look_at(from, to)
    local d = sub(to, from)
    local yaw = math.deg(math.atan(d.y, d.x))
    local pitch = math.deg(math.atan(d.z, hlen(d)))
    return { pitch = pitch, yaw = yaw }
end

function C.new(env)
    return setmetatable({ env = env, mode = "off", target = nil, cam = nil, cam_key = nil,
                          pos = nil, lost_at = nil, orbit = 0, viewing = false }, C)
end

-- The world changed (or the guard dropped its caches): forget the camera.
function C:drop(why)
    if self.cam then self.env.log("spectate cam: dropped (%s)", tostring(why)) end
    self.cam, self.cam_key, self.pos, self.viewing = nil, nil, nil, false
end

function C:follow(name)
    if self.mode ~= "follow" or self.target ~= name then
        self.mode, self.target, self.lost_at, self.viewing = "follow", name, nil, false
        self.snap = true   -- no smoothing across a cut (it would fly through walls)
        self.env.log("spectate cam: follow %s", tostring(name))
    end
end

function C:sky()
    if self.mode ~= "sky" then
        self.mode, self.target, self.viewing = "sky", nil, false
        self.env.log("spectate cam: arena view")
    end
end

function C:off()
    self.mode, self.target, self.lost_at, self.viewing = "off", nil, nil, false
end

-- Arena frame: data first, else the bounds of the living players.
function C:frame_of_arena()
    local a = self.env.arena and self.env.arena()
    if a and a.centre then return a.centre, math.max(a.radius or 0, C.SKY_MIN_R) end
    local ps = self.env.players and self.env.players() or {}
    if #ps == 0 then return nil end
    local c = v(0, 0, 0)
    for _, p in ipairs(ps) do c = add(c, p) end
    c = mul(c, 1 / #ps)
    local r = C.SKY_MIN_R
    for _, p in ipairs(ps) do r = math.max(r, hlen(sub(p, c)) * 1.3) end
    return c, r
end

function C:sky_pose(dt)
    local c, r = self:frame_of_arena()
    if not c then return nil end
    self.orbit = (self.orbit + C.SKY_ORBIT_DPS * dt) % 360
    local a = math.rad(self.orbit)
    -- steep and high: narrow arenas (Alley) show their street, not a roof
    local pos = v(c.x + math.cos(a) * r * C.SKY_ORBIT_R, c.y + math.sin(a) * r * C.SKY_ORBIT_R, c.z + r * 1.3 + 600)
    return pos, C.look_at(pos, c)
end

function C:follow_pose(p, dt)
    local c = self:frame_of_arena()
    -- Behind the target as seen from where the camera is (lazy chase); the
    -- first frame looks from outside the arena towards its centre.
    local from = self.pos
    local dir
    if from then dir = sub(from, p) elseif c then dir = sub(p, c) else dir = v(1, 0, 0) end
    dir.z = 0
    local l = hlen(dir)
    if l < 1 then dir, l = v(1, 0, 0), 1 end
    dir = mul(dir, 1 / l)
    local want = v(p.x + dir.x * C.FOLLOW_DIST, p.y + dir.y * C.FOLLOW_DIST, p.z + C.FOLLOW_UP)
    local pos = want
    if from and not self.snap then
        local k = math.min(1, C.FOLLOW_RATE * dt)
        pos = add(from, mul(sub(want, from), k))
    end
    self.snap = false
    -- Never inside a wall or roof: pull in front of the first world
    -- geometry between the target and the camera (narrow arenas).
    local look = v(p.x, p.y, p.z + C.LOOK_UP)
    local f = self.env.trace and self.env.trace(look, pos)
    if f then pos = add(look, mul(sub(pos, look), math.max(C.MIN_PULL, f * 0.9))) end
    return pos, C.look_at(pos, look)
end

-- One frame. `now` and `dt` in seconds. Returns the mode actually shown.
function C:frame(now, dt)
    if self.mode == "off" then return "off" end
    local env = self.env
    local key = env.world_key()
    if key == nil then self:drop("no world"); return self.mode end
    if self.cam_key ~= key then self:drop("world change") end
    if self.cam and not env.cam_ok(self.cam) then self:drop("camera gone") end

    local pos, rot
    if self.mode == "follow" then
        local p = env.target_pos(self.target)
        if p then
            self.lost_at = nil
            pos, rot = self:follow_pose(p, dt)
        else
            self.lost_at = self.lost_at or now
            if now - self.lost_at >= C.LOST_S then
                env.log("spectate cam: %s gone, arena view", tostring(self.target))
                self:sky()
            elseif self.pos then
                pos, rot = self.pos, self.rot   -- hold for a moment (respawn / stand-in swap)
            end
        end
    end
    if self.mode == "sky" then pos, rot = self:sky_pose(dt) end
    if not pos then return self.mode end

    if not self.cam then
        self.cam = env.spawn_camera(pos, rot)
        if not self.cam then return self.mode end
        self.cam_key, self.viewing = key, false
        env.log("spectate cam: camera spawned")
    else
        env.place(self.cam, pos, rot)
    end
    self.pos, self.rot = pos, rot
    if not self.viewing then
        env.clear_fade()
        env.view(self.cam, C.BLEND_S)
        self.viewing, self.enforced_at = true, now
    elseif env.enforce and now - (self.enforced_at or 0) >= C.ENFORCE_S then
        -- The death flow keeps acting after we take over (a delayed fade, a
        -- death screen, its own view target): hold our view every 100 ms.
        self.enforced_at = now
        env.enforce(self.cam)
    end
    return self.mode
end

return C
