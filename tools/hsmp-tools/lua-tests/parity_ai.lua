-- Drive the real dev takeover loop through a native stand-in possession swap.
local clock = 10
local dir = T.tmpdir("parity_ai_")
local real_getenv = os.getenv
os.getenv = function(k) if k == "HSMP_DEV" then return "1" elseif k == "HSMP_STATE_DIR" then return dir end; return real_getenv(k) end
os.clock = function() return clock end
local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
local READY = dofile(T.path("mods/shared/hsmp_spawn_ready.lua"))
local function obj(name, class)
    local o = { IsValid = function() return true end }
    o.GetFName = function() return { ToString = function() return name end } end
    o.GetClass = function() return obj(class or "Willie_BP_C", "Class") end
    o.K2_GetActorLocation = function() return { X=0,Y=0,Z=100 } end
    return o
end
local me, remote, foreign = obj("Own"), obj("Remote"), obj("Temporary")
local pc = obj("PC", "PlayerController")
pc.Pawn, me.Controller, me.Player = me, pc, true
me["Team Int"], remote["Team Int"] = 1, 101
remote.Health, remote.DED = 100, false
remote.GetActorEnableCollision = function() return true end
remote.Mesh = obj("CharacterMesh0", "SkeletalMeshComponent")
remote.Mesh.IsVisible = function() return true end
remote.Mesh.IsSimulatingPhysics = function() return true end
remote.Mesh.GetCollisionEnabled = function() return 3 end
local takeovers, combat_calls = 0, 0
pc.UnPossess = function(self) if self.Pawn then self.Pawn.Controller = nil end; self.Pawn = nil end
pc.Possess = function(self, pawn) self.Pawn, pawn.Controller = pawn, self end
pc.SetViewTargetWithBlend = function(self, pawn) self.camera = pawn end
me.SpawnDefaultController = function(self)
    takeovers = takeovers + 1
    self.Controller = obj("AI_own", "AI_BP_C")
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
local shown = { peer=2,pawn="Remote",match_id=91,round=1,life=1,local_ms=10000 }
local source = { peer_id=2,has_context=true,match_id=91,round=1,life=1,mode="interp",age=5 }
local bus = { spawn_status=status,puppets={rows={{peer=2,name="Remote"}}},playback={rows={shown}} }
HSMP_IPC = {
    bus_table=function(k) return bus[k] end,
    rec=function(k) return ({session=session,link={my_peer_id=1},mode=raw_mode,local_root=root,vitals=own_v})[k] end,
    peer_rec=function() return peer_v end,
    peer_slot=function() return 0 end,
    peer_play=function(_, out) for k,v in pairs(source) do out[k]=v end; return 1 end,
    sample_status=function() return {pose=pose} end,
}
local guard = {key="world1",pc=function() return pc end,settled=function() return true end,check=function() return true end,on_drop=function() end}
guard.ai_pawn = HW.ai_pawn_lookup
package.preload.UEHelpers = function() return {} end
package.preload.hsmp_wg = function() return {new=function() return guard end,verified_ai_status=HW.verified_ai_status} end
package.preload.hsmp_ipc = function() return {init=function() end} end
package.preload.hsmp_session = function() return {new=function() return {live=function() return true end} end,view=function() return view end,mode=function() return mode end} end
package.preload.hsmp_spawn_ready = function() return READY end
FindAllOf = function() return {me,remote,foreign} end
FName = function(n) return n end
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
remote.DED=false; source.age=-12; advance(); api.ai_tick()
T.check(takeovers == 1 and combat_calls == 1 and pc.Pawn == nil and pc.camera == me, "correct fighter takes over exactly once after all current-life proof")
pc.Pawn, foreign.Controller = foreign, pc
advance(); api.ai_tick()
T.check(takeovers == 1 and HW.ai_pawn_lookup() == me, "stand-in fallback possession keeps verified AI owner and never starts a second AI")
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
