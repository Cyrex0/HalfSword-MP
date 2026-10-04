-- HSMPMenu modes_ui.lua: the GAME MODE screen's state and the lobby's mode line.
--
--   hsmp-tools lua-test modes_ui

local MU = dofile(T.path("mods/HSMPMenu/Scripts/modes_ui.lua"))

local md = { target_s = 90, friendly_fire = true, respawn_s = 5 }
local s = MU.state({ mode = 2, team_rule = 2, teams = 3, round_time_limit_s = 300 }, md,
    { { peer_id = 7, team = 2 }, { peer_id = 9, team = 1 } }, 7)
T.check(s.mode == 2 and s.teams == 3 and s.team_rule == 2 and s.my_team == 2 and s.supported, "state from the config and roster", T.repr(s))
T.check(s.target == 90 and s.ff == true and s.respawn == 5, "options from the mode record")
T.check(MU.summary(s) == "MODE  TEAM ELIMINATION   -   3 TEAMS (PICK)   -   YOU: BLUE   -   5 MIN CLOCK", MU.summary(s))
local old = MU.state({ mode = 0 }, nil, {}, 7)
T.check(not old.supported and MU.summary(old) == "MODE  DUEL   -   (set by the server)", "an older server", MU.summary(old))
T.check(MU.summary(MU.state({ mode = 3 }, { target_s = 60 }, {}, 1)) == "MODE  KING OF THE HILL   -   HOLD 60 S", "the hill target")
T.check(not MU.team_capable(0) and not MU.team_capable(1) and MU.team_capable(6), "teams only in the team-capable modes")
T.check(#MU.MODES == 7 and MU.MODES[7][1] == 6, "every game_mode code has a chip")
