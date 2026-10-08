-- HSMPMenu / modes_ui.lua — the GAME MODE screen (lobby -> MODE) and the lobby's mode line.
--
-- The host (admin) picks the mode, the teams (off / auto-balanced / players pick), the round
-- clock and the mode options; every player picks a team when the teams are PICK (FIXED).
-- Every pick is a command (commands.lua) answered by the server; the chips always show the
-- SERVER's values (the session config, the `mode` record), a pending pick on top.
--
-- The pure helpers (state, summary) are what the offline tests check.

local M = {}
local ctx, Kit

-- game_mode codes (schema session.rs `game_mode`)
M.MODES = {
    { 0, "DUEL", "Last player standing" },
    { 1, "FFA", "Free for all: last player standing" },
    { 2, "TEAMS", "Team elimination: last team standing" },
    { 3, "HILL", "King of the hill: hold the zone alone to score" },
    { 4, "ROULETTE", "Weapon roulette: everyone gets the same random kit each round" },
    { 5, "BRAWL", "Brawl: fists only, no armour" },
    { 6, "DEATHMATCH", "Timed deathmatch: respawns, most kills wins" },
}
M.LABEL = { [0] = "DUEL", [1] = "FREE FOR ALL", [2] = "TEAM ELIMINATION", [3] = "KING OF THE HILL",
            [4] = "WEAPON ROULETTE", [5] = "BRAWL", [6] = "DEATHMATCH" }
M.TEAM_NAMES = { "RED", "BLUE", "GREEN", "GOLD" }
M.RULES = { { 0, "NO TEAMS", "Every player for themselves" }, { 1, "AUTO", "The server balances the teams at START" },
            { 2, "PICK", "Players pick their team below" } }
M.COUNTS = { { 2, "2 TEAMS" }, { 3, "3 TEAMS" }, { 4, "4 TEAMS" } }
M.TIMES = { { 0, "DEFAULT", "No clock (deathmatch 5 min, King of the hill 4 min)" }, { 120, "2 MIN" }, { 180, "3 MIN" },
            { 300, "5 MIN" }, { 600, "10 MIN" } }
M.TARGETS = { { 30, "30 S" }, { 60, "60 S" }, { 90, "90 S" }, { 120, "120 S" } }
M.RESPAWNS = { { 2, "2 S" }, { 3, "3 S" }, { 5, "5 S" }, { 10, "10 S" } }
-- mode_opt codes
M.OPT = { KOTH_TARGET = 1, FRIENDLY_FIRE = 2, RESPAWN_S = 3 }

-- Modes that may have teams (team elimination always does; duel / FFA never).
function M.team_capable(mode) return mode ~= nil and mode >= 2 end

-- Pure: what the screen shows. cfg = the session's live config (mode, team_rule, teams,
-- round_time_limit_s), md = shared/hsmp_session.lua HS.mode() or nil (an older server),
-- roster = session rows (peer_id, team), my_id.
function M.state(cfg, md, roster, my_id)
    cfg = cfg or {}
    local s = { mode = cfg.mode or 0, team_rule = cfg.team_rule or 0, teams = cfg.teams or 0,
                round_time = cfg.round_time_limit_s or 0, supported = md ~= nil, my_team = 0 }
    s.target = md and md.target_s or 60
    s.ff = md and md.friendly_fire or false
    s.respawn = md and md.respawn_s or 3
    for _, r in ipairs(roster or {}) do
        if my_id ~= 0 and r.peer_id == my_id then s.my_team = r.team or 0 end
    end
    return s
end

-- Pure: the lobby's mode line.
function M.summary(s)
    local parts = { "MODE  " .. (M.LABEL[s.mode] or "?") }
    if s.teams > 0 then
        parts[#parts + 1] = string.format("%d TEAMS (%s)", s.teams, s.team_rule == 2 and "PICK" or "AUTO")
        if s.my_team > 0 then parts[#parts + 1] = "YOU: " .. (M.TEAM_NAMES[s.my_team] or "?") end
    end
    if s.round_time > 0 then parts[#parts + 1] = string.format("%d MIN CLOCK", math.floor(s.round_time / 60 + 0.5)) end
    if s.mode == 3 then parts[#parts + 1] = string.format("HOLD %d S", s.target) end
    if not s.supported then parts[#parts + 1] = "(set by the server)" end
    return table.concat(parts, "   -   ")
end

-- --- the screen -----------------------------------------------------------------------------

local b          -- this screen's Kit build
M.cmds = {}      -- kind -> the newest command id

local function send(kind, args)
    M.cmds[kind] = ctx.cmd(kind, args)
    M.render()
end

-- The pending command of `kind`, mapped by `key(args)` (nil = none / not shown).
local function pending(kind, key)
    local st = M.cmds[kind] and ctx.cmd_status(M.cmds[kind])
    if not st or st.state ~= "pending" then return nil end
    return key(st.args)
end

function M.build()
    Kit = ctx.kit
    local L
    L, b = Kit.frame("mode", "GAME MODE", { w = 1500, h = 760 })
    local w = b.w
    local u, F = L.u, L.F
    local gap = L.gap
    local lw = u(250)
    local ch = u(Kit.BH.chip)
    local row_gap = u(Kit.SP.md)
    local y = L.top
    local function row(label, items, cb, help, key)
        Kit.group(key)
        Kit.text(label, L.x0, y, lw - gap, ch, F(Kit.TS.label), 0, Kit.C.text)
        local g = Kit.chip_group(items, L.x0 + lw, y, L.iw - lw, ch, function(v) if ctx.alive() then cb(v) end end,
            { fs = F(Kit.TS.label), gap = gap, help = help })
        y = y + ch + row_gap
        return g
    end
    w.mode = row("MODE", M.MODES, function(v) send("game_mode", { mode = v }) end, "The game mode of the next match", "mode")
    local half = math.floor((L.iw - lw - gap) * 0.55)
    Kit.group("teams")
    Kit.text("TEAMS", L.x0, y, lw - gap, ch, F(Kit.TS.label), 0, Kit.C.text)
    w.rule = Kit.chip_group(M.RULES, L.x0 + lw, y, half, ch, function(v)
        if ctx.alive() then send("teams", { rule = v }) end
    end, { fs = F(Kit.TS.label), gap = gap })
    w.count = Kit.chip_group(M.COUNTS, L.x0 + lw + half + gap, y, L.iw - lw - half - gap, ch, function(v)
        if ctx.alive() then send("teams", { rule = (M.cur and M.cur.team_rule ~= 0) and M.cur.team_rule or 1, n = v }) end
    end, { fs = F(Kit.TS.label), gap = gap, help = "How many teams" })
    y = y + ch + row_gap
    local picks = {}
    for t, n in ipairs(M.TEAM_NAMES) do picks[t] = { t, n } end
    picks[#picks + 1] = { 0, "ANY", "No pick: the server puts you on the smallest team" }
    w.team = row("YOUR TEAM", picks, function(v) send("set_team", { team = v }) end, "Pick your team (PICK teams)", "team")
    w.time = row("ROUND CLOCK", M.TIMES, function(v) send("round_time", { s = v }) end,
        "Round time limit: most standing / most points / most kills wins when it runs out", "time")
    w.target = row("HILL TARGET", M.TARGETS, function(v) send("set_option", { opt = M.OPT.KOTH_TARGET, value = v }) end,
        "King of the hill: seconds on the hill alone that win the round", "target")
    w.ff = row("FRIENDLY FIRE", { { 1, "ON" }, { 0, "OFF" } }, function(v)
        send("set_option", { opt = M.OPT.FRIENDLY_FIRE, value = v })
    end, "Team modes: can teammates hurt each other", "ff")
    w.respawn = row("RESPAWN DELAY", M.RESPAWNS, function(v) send("set_option", { opt = M.OPT.RESPAWN_S, value = v }) end,
        "Deathmatch: seconds from a death to the respawn", "respawn")
    w.about = Kit.text("", L.x0, y, L.iw, u(30), F(Kit.TS.small), 0, Kit.C.dim)
    local acts = Kit.actions(L, nil, { { label = "BACK", key = "back", cb = function() ctx.back() end, help = "Back to the lobby" } })
    w.back = acts.back
    b.on_back = function() ctx.back() end
    Kit.focus_default(w.mode.by_key[0])
    M.render()
end

function M.render()
    if not b or not ctx or not ctx.alive() then return end
    local w = b.w
    local s = M.state(ctx.config(), ctx.mode(), ctx.roster(), ctx.my_peer_id())
    M.cur = s
    local admin, lobby = ctx.is_admin(), ctx.in_lobby()
    local host_off = (not admin) or (not lobby) or not s.supported
    local teams_on = s.teams > 0
    Kit.chip_group_set(w.mode, s.mode, host_off)
    Kit.chip_group_pending(w.mode, pending("game_mode", function(a) return a.mode ~= s.mode and a.mode or nil end))
    Kit.chip_group_set(w.rule, teams_on and s.team_rule or 0,
        (host_off or not M.team_capable(s.mode)) or (s.mode == 2 and { [0] = true } or false))
    Kit.chip_group_set(w.count, teams_on and s.teams or nil, host_off or not teams_on)
    Kit.chip_group_set(w.team, s.my_team, (not lobby) or (not s.supported) or s.team_rule ~= 2)
    Kit.chip_group_pending(w.team, pending("set_team", function(a) return a.team ~= s.my_team and a.team or nil end))
    Kit.chip_group_set(w.time, s.round_time, host_off)
    Kit.chip_group_set(w.target, s.target, host_off or s.mode ~= 3)
    Kit.chip_group_set(w.ff, s.ff and 1 or 0, host_off or not teams_on)
    Kit.chip_group_set(w.respawn, s.respawn, host_off or s.mode ~= 6)
    local why = ((not s.supported) and "This server's HSMP has no game modes")
        or ((not lobby) and "The match is running: the mode changes in the lobby")
        or ((not admin) and "Only the host changes the mode") or nil
    for _, g in ipairs({ w.mode, w.rule, w.count, w.time, w.target, w.ff, w.respawn }) do Kit.chip_group_reason(g, why) end
    Kit.chip_group_reason(w.team, (s.team_rule ~= 2) and "Players pick their team only with PICK teams" or why)
    local about = M.MODES[s.mode + 1] and M.MODES[s.mode + 1][3] or ""
    Kit.set_text(w.about, M.summary(s) .. "   -   " .. about)
    -- the newest command's state on the message line
    local note, col
    for _, kind in ipairs({ "game_mode", "teams", "set_team", "round_time", "set_option" }) do
        local st = M.cmds[kind] and ctx.cmd_status(M.cmds[kind])
        if st and st.state == "refused" and (st.done_age_s or 99) < 6 then note, col = "REFUSED - " .. tostring(st.reason), Kit.C.bad end
        if st and st.state == "pending" then note, col = "WAITING FOR SERVER...", Kit.C.ok end
    end
    Kit.msg(note or why or "Picks apply to the next match.", col or Kit.C.dim)
end

function M.forget() b = nil end

-- c: { kit, cmd(kind, args) -> id, cmd_status(id), config() -> the live session config,
--      mode() -> HS.mode(), roster() -> session rows, my_peer_id(), is_admin(), in_lobby(),
--      alive() -> the screen is up, back() }
function M.attach(c)
    ctx = c
    Kit = c.kit
end

return M
