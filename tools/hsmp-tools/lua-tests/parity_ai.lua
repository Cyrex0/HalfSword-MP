-- Drive the real dev takeover loop through a native stand-in possession swap.
local clock = 10
local dir = T.tmpdir("parity_ai_")
local real_getenv = os.getenv
os.getenv = function(k) if k == "HSMP_DEV" then return "1" elseif k == "HSMP_STATE_DIR" then return dir end; return real_getenv(k) end
os.clock = function() return clock end
local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
local READY = dofile(T.path("mods/shared/hsmp_spawn_ready.lua"))
local next_address = 100
local function obj(name, class)
    next_address = next_address + 1
    local address = next_address
    local o = { IsValid = function() return true end }
    o.GetAddress = function() return address end
    o.GetFName = function() return { ToString = function() return name end } end
    o.GetClass = function() return obj(class or "Willie_BP_C", "Class") end
    o.K2_GetActorLocation = function() return { X=0,Y=0,Z=100 } end
    return o
end
local me, remote, foreign = obj("Own"), obj("Remote"), obj("Temporary")
local world = obj("Arena", "World")
world.GetFullName = function() return "World /Game/Maps/Arena.Arena" end
local native_world = tostring(world:GetAddress()) .. "@" .. world:GetFullName()
local pc = obj("PC", "PlayerController")
pc.Pawn, me.Controller, me.Player = me, pc, true
local input_ignored=false
pc.IsMoveInputIgnored=function()return input_ignored end
pc.GetWorld=function()return world end
me["Team Int"], remote["Team Int"] = 1, 101
remote.Health, remote.DED = 100, false
remote.GetActorEnableCollision = function() return true end
remote.Mesh = obj("CharacterMesh0", "SkeletalMeshComponent")
remote.Mesh.IsVisible = function() return true end
remote.Mesh.IsSimulatingPhysics = function() return true end
remote.Mesh.GetCollisionEnabled = function() return 3 end
local takeovers, combat_calls, stops, brain_stops, brain_restarts = 0, 0, 0, 0, 0
pc.UnPossess = function(self) if self.Pawn then self.Pawn.Controller = nil end; self.Pawn = nil end
pc.Possess = function(self, pawn) self.Pawn, pawn.Controller = pawn, self end
pc.SetViewTargetWithBlend = function(self, pawn) self.camera = pawn end
me.SpawnDefaultController = function(self)
    takeovers = takeovers + 1
    self.Controller = obj("AI_own", "AI_BP_C")
    local c = self.Controller
    c.ticking = true
    c.IsActorTickEnabled = function(self) return self.ticking end
    c.SetActorTickEnabled = function(self, ticking) self.ticking = ticking end
    c.StopMovement = function() stops = stops + 1 end
    c.BrainComponent = obj("Brain", "BrainComponent")
    c.BrainComponent.running = true
    c.BrainComponent.IsRunning = function(self) return self.running end
    c.BrainComponent.StopLogic = function(self, reason) T.check(reason == "HSMP duel is not Live", "native brain receives an explicit duel intent reason"); brain_stops = brain_stops + 1; self.running = false end
    c.BrainComponent.RestartLogic = function(self) brain_restarts = brain_restarts + 1; self.running = true end
    self.Controller["Event Initialize AI"] = function() end
    self.Controller["Event Get Into Combat State"] = function(_, _, target) combat_calls = combat_calls + 1; T.check(target == remote, "native combat uses the bound remote actor") end
end
foreign.SpawnDefaultController = function() error("temporary pawn must never receive an AI takeover") end
local status = { verified=true,pawn="Own",match_id=91,round=1,life=1,spawn_id=256 }
local session = { match_id=91,round=1,phase=3,rows={{peer_id=1,spawn_id=256}} }
local raw_mode = { match_id=91,round=1,rows={{peer_id=1,life=1},{peer_id=2,life=1}} }
local view = { state="live",match_id=91,round=1,my_peer_id=1 }
local mode = { match_id=91,round=1,rows={[1]={life=1,alive=true,team=0},[2]={life=1,alive=true,team=0}} }
local root = { tick=1,ts=10000,match_id=91,round=1,life=1 }
local pose = { tick=1,ts=10000,match_id=91,round=1,life=1 }
local own_v = { seq=1,match_id=91,round=1,life=1,flags=0,v={6400} }
local peer_v = { seq=1,match_id=91,round=1,life=1,flags=0,v={6400} }
local shown = { peer=2,pawn="Remote",match_id=91,round=1,life=1,local_ms=10000,
    settle_world=native_world,settle_sample_ms=10000,settle_stable_ms=150,settle_source_ts=9900,
    settle_source_seq=20,settle_cut=7,settle_ready=true,settle_count=6,settle_pos_uu=1,settle_rot_deg=1,settle_reason="settled" }
local source = { peer_id=2,has_context=true,match_id=91,round=1,life=1,mode="interp",age=5,cut=7 }
local bus = { spawn_status=status,puppets={rows={{peer=2,name="Remote"}}},playback={rows={shown}} }
HSMP_IPC = {
    bus_table=function(k) return bus[k] end,
    rec=function(k) return ({session=session,link={my_peer_id=1},mode=raw_mode,local_root=root,vitals=own_v})[k] end,
    peer_rec=function() return peer_v end,
    peer_slot=function() return 0 end,
    peer_play=function(_, out) for k,v in pairs(source) do out[k]=v end; return 1 end,
    sample_status=function() return {pose=pose} end,
}
local drops, settled = {}, true
local guard = {key="world1",world=function() return world end,pc=function() return pc end,settled=function() return settled end,check=function() return true end,on_drop=function(fn) drops[#drops+1] = fn end}
guard.ai_pawn = HW.ai_pawn_lookup
package.preload.UEHelpers = function() return {} end
package.preload.hsmp_wg = function() return {new=function() return guard end,verified_ai_status=HW.verified_ai_status} end
package.preload.hsmp_ipc = function() return {init=function() end} end
package.preload.hsmp_session = function() return {new=function() return {live=function() return true end} end,view=function() return view end,mode=function() return mode end} end
package.preload.hsmp_spawn_ready = function() return READY end
FindAllOf = function() return {me,remote,foreign} end
FName = function(n) return n end
FString = function(s) return s end
LoopAsync = function() end
RegisterHook = function() end
HSMP_PARITY_TEST = {}
dofile(T.path("mods/dev/HSMPParity/Scripts/main.lua"))
local api = HSMP_PARITY_TEST
api.ai("auto"); api.ai_tick()
T.check(takeovers == 0, "auto waits for actual advancing pose and vitals")
local function advance()
    clock = clock + .05
    root.ts, pose.ts, shown.local_ms = clock*1000,clock*1000,clock*1000
    shown.settle_sample_ms = clock*1000
    root.tick, pose.tick = root.tick+1,pose.tick+1
    own_v.seq, peer_v.seq = own_v.seq+1,peer_v.seq+1
end
advance(); shown.life = 2; api.ai_tick()
T.check(takeovers == 0, "a displayed different life prevents combat even with fresh pose")
shown.life=1; remote["Team Int"]=1; advance(); api.ai_tick()
T.check(takeovers == 0, "matching native teams prevent combat before team initialization")
remote["Team Int"]=101; source.mode="stale"; advance(); api.ai_tick()
T.check(takeovers == 0, "republished stale source cannot start combat")
source.mode="interp"; remote.DED=true; advance(); api.ai_tick()
T.check(takeovers == 0, "native DED flag prevents takeover despite pinned HP")
remote.DED=false; source.age=-12; shown.settle_ready=false; shown.settle_reason="hand_r rotation"; advance(); api.ai_tick()
T.check(takeovers == 0, "a physically twisted spawned hand prevents AI handover despite fresh pose, health and collision")
shown.settle_ready=true; shown.settle_stable_ms=149; advance(); api.ai_tick()
T.check(takeovers == 0, "AI waits for continuous actual limb stability")
shown.settle_stable_ms=150; input_ignored=true; advance(); api.ai_tick()
T.check(takeovers==0,"server Live and healthy limbs cannot bypass the local first-input gate")
input_ignored=nil; advance(); api.ai_tick()
T.check(takeovers==0,"unavailable native input state never invents an AI release")
local input_get=pc.IsMoveInputIgnored
pc.IsMoveInputIgnored=function()error("input getter unavailable")end; advance(); api.ai_tick()
T.check(takeovers==0,"a throwing native input getter refuses takeover")
pc.IsMoveInputIgnored=function()pc.Pawn=foreign;return false end; advance(); api.ai_tick()
T.check(takeovers==0,"input getter possession reentry cannot hand a temporary pawn to AI")
pc.Pawn=me;pc.IsMoveInputIgnored=input_get
local reused=obj("ReusedPawn");reused.GetAddress=me.GetAddress
pc.IsMoveInputIgnored=function()pc.Pawn=reused;return false end;advance();api.ai_tick()
T.check(takeovers==0,"a reused address with a different native pawn name cannot borrow input release")
pc.Pawn=me;pc.IsMoveInputIgnored=input_get
local pc_world_get=pc.GetWorld
local old_pawn_reads=0
local pawn_get_addr=me.GetAddress
me.GetAddress=function()old_pawn_reads=old_pawn_reads+1;return pawn_get_addr()end
local replacement_world=obj("ReplacementWorld","World")
replacement_world.GetFullName=world.GetFullName
pc.GetWorld=function()return replacement_world end;input_ignored=false;advance();api.ai_tick()
T.check(takeovers==0,"fresh native controller world loss refuses despite unchanged cached world key")
T.check(old_pawn_reads==1,"fresh native world mismatch stops before any post-getter old pawn identity read")
me.GetAddress=pawn_get_addr
pc.GetWorld=pc_world_get
input_ignored=false; advance(); api.ai_tick()
T.check(takeovers == 1 and combat_calls == 1 and pc.Pawn == nil and pc.camera == me, "correct fighter takes over exactly once after all current-life proof")
pc.Pawn, foreign.Controller = foreign, pc
advance(); api.ai_tick()
T.check(takeovers == 1 and HW.ai_pawn_lookup() == me, "stand-in fallback possession keeps verified AI owner and never starts a second AI")
-- A native reason-2 yield changes phase while the physical fighter remains alive.
-- Control intent must stop without changing its life or native simulation.
local c = me.Controller
me.Health, me.HeadHealth, me.DED, me.Bleeding = 78.86, 41, false, 2.5
view.state, session.phase = "roundover", 4
api.ai_tick()
T.check(c.ticking == false and stops == 1 and brain_stops == 1 and c.BrainComponent.running == false,
    "RoundOver immediately stops movement, native decision tick and active brain")
T.check(me.Controller == c and me.Player == false and me.Health == 78.86 and me.HeadHealth == 41 and me.DED == false and me.Bleeding == 2.5,
    "yield pause keeps possession, native health and continuing wounds unchanged")
advance(); api.ai_tick()
T.check(stops == 1 and brain_stops == 1 and takeovers == 1 and combat_calls == 1,
    "repeated RoundOver ticks neither reinitialize combat nor repeat native stop calls")
view.state, session.phase = "live", 3
api.ai_tick()
T.check(c.ticking == true and brain_restarts == 1 and c.BrainComponent.running == true and takeovers == 1,
    "same-life Live resumes original native controller and stopped brain immediately")
view.state, session.phase = "paused", 7
api.ai_tick()
T.check(c.ticking == false and brain_stops == 2, "a Paused duel also suspends verified same-life AI intent")
view.state, session.phase = "live", 3
api.ai_tick()
T.check(c.ticking == true and brain_restarts == 2, "unpausing resumes the exact current fighter")
mode.rows[1].alive = false; api.ai_tick()
T.check(c.ticking == false and brain_stops == 3, "a current-life yield stops AI while the Live phase notification is in flight")
mode.rows[1].alive = true; api.ai_tick()
T.check(c.ticking == true and brain_restarts == 3, "only a current authoritative alive life may resume Live intent")
-- Do not start a brain or tick that was already off for native reasons.
c.ticking, c.BrainComponent.running = false, false
view.state, session.phase = "roundover", 4; api.ai_tick()
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == false and brain_stops == 3 and brain_restarts == 3,
    "resume preserves native-disabled tick and never starts a previously stopped brain")
c.ticking, c.BrainComponent.running = true, true
-- Blueprint-only controllers have no BrainComponent; their ReceiveTick still stops.
local brain = c.BrainComponent
c.BrainComponent = nil
view.state, session.phase = "roundover", 4; api.ai_tick()
T.check(c.ticking == false, "a controller without a native brain still stops Blueprint combat decisions")
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == true and brain_restarts == 3, "brain absence is not filled with a synthetic start")
c.BrainComponent = brain
-- Native readback, not a successful Lua invocation, proves the decision tick stopped.
local set_tick = c.SetActorTickEnabled
c.SetActorTickEnabled = function(self, enabled) if enabled then self.ticking = true end end
view.state, session.phase = "roundover", 4; api.ai_tick()
T.check(c.ticking == true and brain_stops == 4, "a no-effect native tick setter cannot be treated as paused")
c.SetActorTickEnabled = set_tick
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == true and brain_restarts == 4, "Live reverses a partial pause immediately without waiting for its failure retry")
local read_tick, stopped_before = c.IsActorTickEnabled, stops
c.IsActorTickEnabled = function() error("tick state unavailable") end
view.state, session.phase = "roundover", 4; api.ai_tick()
T.check(stops == stopped_before and c.ticking == true and brain_stops == 4,
    "unreadable native tick state refuses mutation rather than inventing a restore state")
c.IsActorTickEnabled = read_tick
view.state, session.phase = "live", 3
-- An old generation must never be reactivated, even in the same world.
view.state, session.phase = "roundover", 4; api.ai_tick()
raw_mode.rows[1].life = 2; mode.rows[1].life = 2
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == false and brain_restarts == 4, "new authoritative life never resumes the paused old controller")
raw_mode.rows[1].life, mode.rows[1].life = 1, 1
c.ticking, brain.running = true, true
view.state, session.phase = "roundover", 4; api.ai_tick()
for _, drop in ipairs(drops) do drop("travel") end
guard.key = "world2"; settled = false
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == false and brain_restarts == 4, "travel drops pause state without touching the old native controller")
settled = true; guard.key = "world1"
-- A replacement brain is not the brain that received StopLogic.
c.ticking, brain.running = true, true
view.state, session.phase = "roundover", 4; api.ai_tick()
c.BrainComponent = obj("ReplacementBrain", "BrainComponent")
c.BrainComponent.IsRunning = function() return false end
c.BrainComponent.RestartLogic = function() error("replacement brain must not be started") end
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == true and brain_restarts == 4, "same fighter may resume tick without restarting a replacement brain")
c.BrainComponent = brain
brain.running = true
view.state, session.phase = "roundover", 4; api.ai_tick()
local replacement = obj("AI_own", "AI_BP_C")
replacement.ticking = true
replacement.SetActorTickEnabled = function() error("replacement controller must not inherit old pause state") end
me.Controller = replacement
view.state, session.phase = "live", 3; api.ai_tick()
T.check(c.ticking == false and replacement.ticking == true and brain_restarts == 4,
    "a controller with a reused name cannot inherit or resume a different native controller's state")
me.Controller = c
session.phase=7
T.check(HW.ai_pawn_lookup() == me, "Paused retains the verified AI fighter's current round and Mode life")
raw_mode.rows[1].life=2
T.check(HW.ai_pawn_lookup() == nil, "Paused rejects an old AI fighter after the authoritative life changes")
session.phase=3; raw_mode.rows[1].life=1
raw_mode.rows[1].life=2
T.check(HW.ai_pawn_lookup() == nil, "same-world new native life rejects the old AI pawn")
raw_mode.rows[1].life=1; session.match_id=92
T.check(HW.ai_pawn_lookup() == nil, "new server match rejects old verified AI placement")
session.match_id=91; session.rows[1].spawn_id=257
T.check(HW.ai_pawn_lookup() == nil, "different authoritative spawn order rejects old AI placement")
session.rows[1].spawn_id=256; me.Player=true
T.check(HW.ai_pawn_lookup() == nil, "a player-held fighter is never resolved as an AI-owned fighter")
me.Player=false; me.Controller=obj("WrongController","AIController")
T.check(HW.ai_pawn_lookup() == nil, "only the exact native AI_BP_C controller proves ownership")
