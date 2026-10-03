-- Offline test of HSMPAvatars PURE (servo / advance / parse_play2).
local src = (io.open(arg[1], "rb"):read("a")):gsub(string.char(13), "")
local block = src:match("%-%- BEGIN PURE.-\n(.-)%-%- =+\n%-%- END PURE")
assert(block, "PURE block not found")
local PURE = load("local PURE = {}\n" .. block:gsub("^local PURE = {}\n", "") .. "\nreturn PURE")()
local function q_axis(ax, deg)
  local l = math.sqrt(ax[1]^2 + ax[2]^2 + ax[3]^2); local h = math.rad(deg) / 2
  return { ax[1] / l * math.sin(h), ax[2] / l * math.sin(h), ax[3] / l * math.sin(h), math.cos(h) }
end
-- free rigid body integration: COM moves with v, rotation about COM with w (deg/s, world)
local function step(cur, com, vx, vy, vz, wx, wy, wz, dt)
  local cq = { cur[4], cur[5], cur[6], cur[7] }
  local cw = PURE.qrot(cq, com)
  local C = { cur[1] + cw[1] + vx * dt, cur[2] + cw[2] + vy * dt, cur[3] + cw[3] + vz * dt }
  local ax, ay, az = math.rad(wx) * dt, math.rad(wy) * dt, math.rad(wz) * dt
  local a = math.sqrt(ax * ax + ay * ay + az * az)
  local nq = cq
  if a > 1e-12 then local h = math.sin(a / 2) / a; nq = PURE.qmul({ ax * h, ay * h, az * h, math.cos(a / 2) }, cq) end
  local nw = PURE.qrot(nq, com)
  return { C[1] - nw[1], C[2] - nw[2], C[3] - nw[3], nq[1], nq[2], nq[3], nq[4] }
end
local com = { 7.0, -3.0, 2.5 }
local cur = { 100, 50, 80, table.unpack(q_axis({ 0.2, 1, 0.1 }, 40)) }
local tq = q_axis({ 1, 0.3, -0.2 }, 75)
local tg = { 112, 47, 83, tq[1], tq[2], tq[3], tq[4], 700, -180, 180, 0, 0, 0 }
local vx, vy, vz, wx, wy, wz = PURE.servo(cur, tg, com, 1 / 100, 1e9, 1e9)
local nx = step(cur, com, vx, vy, vz, wx, wy, wz, 1 / 100)
local e = PURE.d3(nx, tg)
local ang = PURE.qangle({ nx[4], nx[5], nx[6], nx[7] }, tq)
print(string.format("servo one step: pos err %.5f uu, rot err %.5f deg", e, ang))
assert(e < 1e-3 and ang < 1e-3)
-- capped: correction limited relative to the replicated velocity
local vx2, vy2, vz2, _, _, _, dl = PURE.servo(cur, tg, com, 1 / 100, 100, 100)
assert(dl > 100)
-- advance with angular velocity: 90 deg/s about z for 0.5 s = 45 deg
local a = PURE.advance({ 0, 0, 0, 0, 0, 0, 1, 10, 0, 0, 0, 0, 90 }, 500)
assert(math.abs(a[1] - 5) < 1e-9 and math.abs(PURE.qangle({ a[4], a[5], a[6], a[7] }, q_axis({ 0, 0, 1 }, 45))) < 1e-6)
-- parse a v2 play line (2 slots: pelvis + weapon_r)
local line = '{"peer_id":2,"seq":9,"pt":123,"mode":"interp","age":20,"delay":27.5,"jit":3.1,"cut":0,"root":[1.0,2.0,3.0,90.00],"v":2,"ptf":123.45,"m":' ..
  tostring(1 | (1 << 23)) .. ',"vm":1,"B":[1,2,3,0,0,0,1,4,5,6,7,8,9,10,11,12,0,0,0,1,0,0,0,0,0,0],"W":[1,7,0,0,15,0,0,100],"C":[5,2,0,1,2,3],"end":1}'
local t = PURE.parse_play2(line, nil)
assert(type(t) == "table" and t.v2 and t.seq == 9 and math.abs(t.pt - 123.45) < 1e-9)
assert(t.slots[1][8] == 4 and t.slots[24][1] == 10 and t.nbones == 1)
assert(t.weapons[24][2] == 7 and t.weapons[24][8] == 100 and t.control[1] == 5)
assert(PURE.parse_play2(line, 9) == "same")
assert(PURE.parse_play2(line:gsub(',"end":1}', ''), nil) == nil)
print("PURE servo/advance/parse_play2: ok")
-- fk_retarget: child positions rebuilt from the parent target + the stand-in's offset
do
  local tg = {}
  for i = 1, PURE.V2_NB do tg[i] = { 0, 0, 0, 0, 0, 0, 1, 1, 2, 3, 0, 0, 0 } end
  tg[1] = { 10, 20, 30, table.unpack(q_axis({ 0, 0, 1 }, 90)) }
  tg[1][8], tg[1][9], tg[1][10], tg[1][11], tg[1][12], tg[1][13] = 0, 0, 0, 0, 0, 0
  local loc = {}
  for i = 2, PURE.V2_NB do loc[i] = { 5, 0, 0 } end
  local r = PURE.fk_retarget(tg, loc)
  -- spine_01 (parent pelvis, rotated 90 deg about z): +5 x -> +5 y
  assert(math.abs(r[2][1] - 10) < 1e-9 and math.abs(r[2][2] - 25) < 1e-9 and math.abs(r[2][3] - 30) < 1e-9)
  -- spine_02's parent spine_01 has identity rotation in tg: +5 x from (10,25,30)
  assert(math.abs(r[3][1] - 15) < 1e-9 and math.abs(r[3][2] - 25) < 1e-9)
  assert(r[3][8] == 1 and r[3][10] == 3)   -- velocities kept
  assert(tg[3][1] == 0)                    -- input untouched
  print("PURE fk_retarget: ok")
end
