-- HSMPHud / hud_model.lua — turns a state snapshot (hud_state.read_all) into
-- what the HUD shows. Pure Lua: no UObject, no file I/O, the clock is passed
-- in. The tracker T keeps the few facts that need history (phase edges, the
-- FIGHT! flash, when the link was lost, the kill feed).

local Mo = {}

Mo.FIGHT_FLASH_S = 1.5     -- "FIGHT!" after countdown -> live
Mo.DIED_S        = 3.0     -- "YOU DIED", then "SPECTATING"
Mo.ARENA_VIEW    = 0xFFFFFFFF  -- spectate target: HSMPMatch shows the arena overview
Mo.FEED_LINES    = 5
Mo.FEED_TTL_S    = 6.0     -- a kill-feed line stays, then fades over FEED_FADE_S
Mo.FEED_FADE_S   = 1.0
Mo.MP_ALIVE_S    = 15      -- the sidecar heartbeat / a link change must be this recent (same rule as HSMPMatch)
Mo.METRICS_STALE_S = 5     -- metrics sample unchanged this long while connected -> NO DATA
Mo.HP_MAX, Mo.ST_MAX = 100, 100
Mo.PREFS_DEFAULT = { hud = true, killfeed = true, net = "always" }   -- .settings.json HUD switches

-- Net grade, tuned for internet play (a clean 116 ms link must not read
-- "NET BAD"). The figure shown is the round-trip time (the sidecar's
-- ping/pong rtt_ms, i.e. what players call "ping"). Four grades:
--   RTT   good < 80 ms, ok < 150 ms, poor < 250 ms, bad above
--   loss  (the 10 s window loss_pct_10s; lifetime loss_pct only as a
--         fallback): < 1 % no effect, 1-3 % at best ok, 3-8 % at best poor,
--         > 8 % bad
--   jitter (jitter_ms = the transport's RTT variation, rttvar, which runs
--         higher than packet jitter, and starts at RTT/2): > 50 ms one grade
--         worse, > 120 ms two
--   no datagram for 1.5 s: bad.  Metrics sample unchanged 5 s: NET BAD NO DATA.
Mo.NET = {
    good_rtt = 80, ok_rtt = 150, poor_rtt = 250,
    ok_loss = 1, poor_loss = 3, bad_loss = 8,
    jitter1 = 50, jitter2 = 120,
    bad_rx_ms = 1500,
}
Mo.NET_LEVELS = { "good", "ok", "poor", "bad" }
Mo.NET_WORD = { good = "NET GOOD", ok = "NET OK", poor = "NET POOR", bad = "NET BAD" }

-- The RTT the grade uses: the ping/pong round trip (rtt_ms). The reliable
-- layer's srtt_ms is not used: it includes ack delay.
function Mo.net_rtt(mt) return mt.rtt end

-- Pure: RTT (ms), loss (%, or nil), jitter (ms, or nil), rx_late -> level.
function Mo.net_grade(rtt, loss, jitter, rx_late)
    local N = Mo.NET
    if rx_late then return "bad" end
    local g = 1
    if rtt >= N.poor_rtt then g = 4 elseif rtt >= N.ok_rtt then g = 3 elseif rtt >= N.good_rtt then g = 2 end
    if jitter then
        if jitter > N.jitter2 then g = g + 2 elseif jitter > N.jitter1 then g = g + 1 end
    end
    if loss then
        local floor = 1
        if loss > N.bad_loss then floor = 4 elseif loss >= N.poor_loss then floor = 3 elseif loss >= N.ok_loss then floor = 2 end
        if floor > g then g = floor end
    end
    if g > 4 then g = 4 end
    return Mo.NET_LEVELS[g]
end

local SHOWN  = { countdown = true, live = true, roundover = true, paused = true, match_over = true }
local ACTIVE = { countdown = true, live = true, roundover = true, paused = true }

function Mo.new()
    return {
        last_state = nil, fight_until = -1, live_since = nil, died_at = nil, died_round = nil,
        in_match = false, link_lost_at = nil,
        mp = { line = nil, seen = false, changed_at = -1e9, active = false },
        metrics = { line = nil, changed_at = -1e9 },
        feed = {},
        tab = false,
        -- panels (hud_panel.lua): the modal seq the player answered, the MP
        -- pause, a pending confirm, the result-screen choice
        conn = nil, dismissed_seq = nil, pause_open = false, confirm = nil, confirm_at = 0,
        result_choice = nil, result_key = nil,
    }
end

Mo.CONFIRM_S = 4.0         -- a "confirm" button reverts after this long

-- --- names ------------------------------------------------------------------------------

local function my_id(snap) return snap.sc and snap.sc.my_id or 0 end

function Mo.nick(snap, id)
    if id == my_id(snap) and id ~= 0 then
        return snap.my_nick or (snap.sc and snap.sc.nicks[id]) or "You"
    end
    return (snap.sc and snap.sc.nicks[id]) or ("P" .. tostring(id))
end

-- who for prose: "You" for me
local function who(snap, id)
    if id == my_id(snap) and id ~= 0 then return "You" end
    return Mo.nick(snap, id)
end

function Mo.arena_label(a)
    if not a or a == "" then return "" end
    local s = a:gsub("^Map_Arena_", ""):gsub("_", " ")
    s = s:gsub("(%l)(%u)", "%1 %2")
    return s
end

local function mmss(sec)
    sec = math.max(0, math.floor(sec + 0.0001))
    return string.format("%d:%02d", math.floor(sec / 60), sec % 60)
end
Mo.mmss = mmss

-- --- observe: update the tracker from a fresh snapshot -----------------------------------

-- ctx = { in_arena = bool }
function Mo.observe(T, snap, now, ctx)
    -- MP session freshness: the sidecar's heartbeat in the
    -- segment header (snap.hb_age, s), never "the status changed recently" (the
    -- link record changes on change only). sidecar_raw is the link record's
    -- version: a CHANGE of it is only evidence for the rejected banner.
    local raw = snap.sidecar_raw
    if not T.mp.seen then
        T.mp.seen, T.mp.line = true, raw
    elseif raw ~= T.mp.line then
        T.mp.line, T.mp.changed_at = raw, now
    end
    local st = snap.sc and snap.sc.status
    local ss = snap.sess
    if ss then
        -- The shared liveness rule (shared/hsmp_session.lua, polled by
        -- main.lua): connected = status "connected" AND a fresh header heartbeat;
        -- a "reconnecting" sidecar counts as an MP session while it still beats.
        T.connected = ss.live == true
        T.mp.active = T.connected or (st == "reconnecting" and ss.fresh == true)
        T.mp.dead = ss.fresh ~= true
        -- The rejected banner needs a live sidecar (its header
        -- heartbeat): a "rejected" link left behind by a sidecar that is gone
        -- never shows in single-player.
        if snap.rejected and not ss.fresh then snap.rejected = nil end
    else
        -- pure-model callers (tests) without a session tracker: the same rule on
        -- the snapshot's heartbeat age
        local beat = snap.hb_age ~= nil and snap.hb_age < Mo.MP_ALIVE_S
        T.connected = (st == "connected") and beat
        T.mp.active = (st == "connected" or st == "reconnecting") and beat
        T.mp.dead = not beat
        if snap.rejected and not beat then snap.rejected = nil end
    end
    T.conn = snap.conn
    -- a new modal (higher seq) replaces an answered one
    if T.conn and T.dismissed_seq and T.conn.seq > T.dismissed_seq then T.dismissed_seq = nil end

    if snap.metrics_raw ~= T.metrics.line then
        T.metrics.line, T.metrics.changed_at = snap.metrics_raw, now
    end

    local m = snap.match
    local state = m and m.state or nil
    if state ~= T.last_state then
        if state == "live" then
            T.live_since = now
            if T.last_state == "countdown" then T.fight_until = now + Mo.FIGHT_FLASH_S end
        elseif state ~= "paused" then
            T.live_since = nil
        end
        T.last_state = state
    end
    -- HUD switches (.settings.json): taken while no match runs, so a change in
    -- SETTINGS applies from the next match, never in the middle of one.
    if snap.prefs and (T.prefs == nil or not (m and SHOWN[m.state])) then T.prefs = snap.prefs end
    if m and ctx.in_arena and ACTIVE[m.state] and T.connected then T.in_match = true end
    if not ctx.in_arena then T.in_match = false end

    -- link lost: only after we were in a running match in this arena
    -- A sidecar whose heartbeat stopped (crashed, killed) is no
    -- session at all, not a lost link that is being reconnected.
    if T.in_match and not T.connected and not T.mp.dead then
        T.link_lost_at = T.link_lost_at or now
    else
        T.link_lost_at = nil
    end

    -- my death this round (server says not alive)
    local me = my_id(snap)
    if m and m.state == "live" and me ~= 0 and m.alive[me] == false then
        if T.died_round ~= m.round then T.died_round, T.died_at = m.round, now end
    elseif m and m.state == "countdown" then
        T.died_round, T.died_at = nil, nil
        T.result_choice, T.result_at = nil, nil   -- a new match (rematch) started
    end
end

-- --- kill feed ---------------------------------------------------------------------------

local CAUSE = { [1] = "", [2] = "", [0] = "" }

function Mo.death_text(snap, d)
    local v = who(snap, d.victim)
    if d.cause == 3 then return v .. " left the fight" end
    if d.cause == 6 then return v .. " surrendered" end
    if d.cause == 5 then
        if d.killer and d.killer ~= 0 and d.killer ~= d.victim then
            return who(snap,d.killer) .. " defeated " .. v
        end
        return v .. (v == "You" and " were defeated" or " was defeated")
    end
    if d.killer and d.killer ~= 0 and d.killer ~= d.victim then
        return who(snap, d.killer) .. " slew " .. v .. (CAUSE[d.cause] or "")
    end
    if v == "You" then return "You died" end
    return v .. " died"
end

function Mo.add_deaths(T, snap, list, now)
    for _, d in ipairs(list or {}) do
        table.insert(T.feed, { text = Mo.death_text(snap, d), at = now,
                               mine = (d.victim == my_id(snap) or d.killer == my_id(snap)) })
    end
    while #T.feed > Mo.FEED_LINES do table.remove(T.feed, 1) end
end

-- Server notices as toasts in the feed ("Mate joined", "Mate left", "The host
-- X left; Y is the host now", "Mate failed to load: ..."). Each event once
-- (hud_state dedups by event_id) and the same text never twice within 5 s.
Mo.TOAST_NAMES = { player_joined = true, player_left = true, host_left = true, load_failed = true,
                   admin_changed = true, sudden_death = true, notice = true }
function Mo.add_notices(T, list, now)
    for _, n in ipairs(list or {}) do
        if Mo.TOAST_NAMES[n.name] then
            local dup = false
            for _, e in ipairs(T.feed) do
                if e.text == n.text and now - e.at < 5 then dup = true end
            end
            if not dup then
                table.insert(T.feed, { text = n.text, at = now, notice = true,
                                       host = (n.name == "host_left") })
            end
        end
    end
    while #T.feed > Mo.FEED_LINES do table.remove(T.feed, 1) end
end

-- --- net indicator ----------------------------------------------------------------------------

function Mo.net(T, snap, now)
    local st = snap.sc and snap.sc.status
    if T.conn and T.conn.state == "reconnecting" then
        return { level = "bad", text = string.format("NO LINK - RECONNECTING %ds", T.conn.remaining or 0) }
    end
    if not T.connected then
        if st == "reconnecting" or T.link_lost_at then return { level = "bad", text = "NO LINK - RECONNECTING" } end
        return { level = "bad", text = "OFFLINE" }
    end
    local mt = snap.metrics
    if not mt or not mt.rtt then return { level = "none", text = "NET --" } end
    local N = Mo.NET
    local stale = (now - T.metrics.changed_at) > Mo.METRICS_STALE_S
    if stale then return { level = "bad", text = "NET BAD  NO DATA" } end
    local rx_late = false
    if mt.last_rx then
        -- an age in ms, or (> 1e9) a UTC timestamp compared with ts_utc_ms
        local age = mt.last_rx
        if age > 1e9 and mt.ts then age = mt.ts - age end
        rx_late = age > N.bad_rx_ms
    end
    local rtt = Mo.net_rtt(mt)
    local level = Mo.net_grade(rtt, mt.loss, mt.jitter, rx_late)
    local parts = { Mo.NET_WORD[level], string.format("%d ms", math.floor(rtt + 0.5)) }
    if mt.loss then parts[#parts + 1] = string.format("%.1f%%", mt.loss) end
    if mt.jitter then parts[#parts + 1] = string.format("+-%d", math.floor(mt.jitter + 0.5)) end
    return { level = level, text = table.concat(parts, "  ") }
end

-- --- game modes (snap.mode: shared/hsmp_session.lua HS.mode()) ---------------------------------

Mo.TEAM_NAME = { "RED", "BLUE", "GREEN", "GOLD" }

-- A team-mode view (nil without teams).
local function teams_of(snap) local md = snap.mode; return md and md.teams > 0 and md or nil end

local function my_team(snap)
    local md = snap.mode
    local r = md and md.rows[my_id(snap)]
    return r and r.team or 0
end

-- Where the hill is from my pawn: "ON THE HILL" or "12 m AHEAD-LEFT". pos = {x, y, z} (cm),
-- yaw (deg, UE: 0 = +X, 90 = +Y). nil without a position.
function Mo.hill_hint(z, pos, yaw)
    if not z or not pos then return nil end
    local dx, dy = z.x - pos[1], z.y - pos[2]
    local d = math.sqrt(dx * dx + dy * dy)
    if d <= z.r and math.abs(pos[3] - z.z) <= (z.hh or 300) then return "ON THE HILL" end
    local m = math.max(1, math.floor((d - z.r) / 100 + 0.5))
    if not yaw then return string.format("%d m", m) end
    local rel = (math.deg(math.atan(dy, dx)) - yaw + 540) % 360 - 180   -- -180..180, + = right
    local dir
    if math.abs(rel) <= 30 then dir = "AHEAD"
    elseif math.abs(rel) >= 150 then dir = "BEHIND"
    elseif rel > 0 then dir = (rel < 90) and "AHEAD-RIGHT" or "RIGHT"
    else dir = (rel > -90) and "AHEAD-LEFT" or "LEFT" end
    return string.format("%d m %s", m, dir)
end

-- Who holds the hill, for the banner.
local function hill_holder(snap)
    local md = snap.mode
    local z = md and md.zone
    if not z then return nil end
    if z.contested then return "CONTESTED" end
    if z.holder_team then
        return (z.holder_team == my_team(snap) and "YOUR TEAM" or Mo.TEAM_NAME[z.holder_team]) .. " HOLDS IT"
    end
    if z.holder_seat then
        local r = md.by_seat[z.holder_seat]
        if r then return (r.peer_id == my_id(snap) and "YOU HOLD IT" or (string.upper(Mo.nick(snap, r.peer_id)) .. " HOLDS IT")) end
    end
    return "EMPTY"
end

-- The round score of a player / team in this mode's units ("23 s", "4 K"), or nil.
local function round_score(md, v)
    if md.id == "koth" then return string.format("%d s", math.floor((v or 0) / 1000)) end
    if md.id == "deathmatch" then return string.format("%d K", v or 0) end
    return nil
end

-- --- centre phase message ---------------------------------------------------------------------

local function score_text(snap)
    local m = snap.match
    local md = teams_of(snap)
    local parts = {}
    if md then
        for t = 1, md.teams do parts[#parts + 1] = string.format("%s %d", Mo.TEAM_NAME[t], md.team_wins[t] or 0) end
        return table.concat(parts, "  -  ")
    end
    for _, id in ipairs(m.order) do parts[#parts + 1] = string.format("%s %d", Mo.nick(snap, id), m.wins[id] or 0) end
    return table.concat(parts, "  -  ")
end

-- "Team Red wins the round" / "You win the round" / "<nick> wins the round".
local function round_winner_text(snap, m)
    local md = snap.mode
    if md and md.winner_team > 0 then
        if md.winner_team == my_team(snap) then return "Your team wins the round" end
        return "Team " .. (md.teams > 0 and Mo.TEAM_NAME[md.winner_team] or "?"):lower():gsub("^%l", string.upper) .. " wins the round"
    end
    return (m.last_winner == my_id(snap)) and "You win the round" or (Mo.nick(snap, m.last_winner) .. " wins the round")
end

local function first_alive_foe(snap)
    local m, me = snap.match, my_id(snap)
    for _, id in ipairs(m.order) do
        if id ~= me and m.alive[id] then return id end
    end
    return nil
end

function Mo.centre(T, snap, now)
    local c = snap.conn
    if c then
        -- The Director's connection state is the only truth
        -- (docs/development/subsystems/director.md).
        if c.state == "reconnecting" then
            return { title = "RECONNECTING...",
                     sub = string.format("the round is frozen for you   -   %d s left", math.max(0, c.remaining or 0)),
                     tone = "bad" }
        end
        -- lost / notice: the modal (hud_panel) shows it, nothing in the centre
        if c.state == "lost" or c.state == "notice" then return nil end
    else
        -- no conn_state record (an older Director)
        if snap.rejected then
            local r = snap.rejected
            if #r > 110 then r = r:sub(1, 107) .. "..." end
            return { title = "CONNECTION REJECTED", sub = r, tone = "bad" }
        end
        if T.link_lost_at then
            return { title = "CONNECTION LOST",
                     sub = string.format("reconnecting... %ds", math.floor(now - T.link_lost_at)), tone = "bad" }
        end
    end
    local m = snap.match
    if not m then return nil end
    if m.state=="live" and snap.surrender then
        return {title=string.format("Hold to surrender %d%%",math.floor(snap.surrender.progress*100)),
            sub="Release to cancel",tone="warn"}
    end
    local me = my_id(snap)
    if m.state == "lobby" and Mo.rematch_waiting(T, now) then
        return { title = "REMATCH", sub = string.format("waiting for everyone to be ready   -   %d s", math.max(0, math.ceil(Mo.REMATCH_HOLD_S - (now - T.result_at)))) }
    end
    if m.state == "countdown" then
        if #m.waiting > 0 then
            local names = {}
            for _, id in ipairs(m.waiting) do names[#names + 1] = Mo.nick(snap, id) end
            return { title = "WAITING FOR PLAYERS", sub = "loading: " .. table.concat(names, ", ") }
        end
        local kit = snap.mode and snap.mode.kit_label ~= "" and ("   -   round kit: " .. snap.mode.kit_label) or ""
        return { title = string.format("ROUND %d", m.round + 1),
                 sub = string.format("%s   -   fight in %d%s", score_text(snap), m.countdown, kit) }
    elseif m.state == "live" then
        if now < T.fight_until then return { title = "FIGHT!", sub = score_text(snap), tone = "good" } end
        if me ~= 0 and m.alive[me] == false then
            local my_hp = snap.vown and snap.vown.hp
            -- who the camera really follows (HSMPMatch's spectate bus record), else the first living foe
            local sp = snap.spectate
            local arena = sp and sp.target == Mo.ARENA_VIEW
            local foe = (not arena) and ((sp and sp.target) or first_alive_foe(snap)) or nil
            local name = (not arena) and ((sp and sp.nick ~= "" and sp.nick) or (foe and Mo.nick(snap, foe))) or nil
            local title = arena and "ARENA VIEW" or (name and ("SPECTATING " .. string.upper(name)) or "SPECTATING")
            local keys = (sp and sp.alive and sp.alive > 1) and "Q / E switch   -   TAB scores" or "TAB scores"
            -- deathmatch: the server respawns us
            local mr = snap.mode and snap.mode.rows[me]
            if mr and mr.respawning then
                local t = mr.respawn_in_ms
                if T.died_at and now - T.died_at < Mo.DIED_S then
                    return { title = "YOU DIED", sub = "respawning soon", tone = "bad" }
                end
                return { title = (t and t > 0) and string.format("RESPAWN IN %d", math.ceil(t / 1000)) or "RESPAWNING...",
                         sub = "loading the arena for your next life   -   TAB scores" }
            end
            if my_hp and my_hp > 0 then
                -- the server has us out but our pawn lives: a late joiner
                return { title = title, sub = "you join the next round   -   " .. keys }
            end
            if T.died_at and now - T.died_at < Mo.DIED_S then
                return { title = "YOU DIED", sub = "the round plays on", tone = "bad" }
            end
            return { title = title, sub = "waiting for the round to end   -   " .. keys }
        end
        return nil
    elseif m.state == "roundover" then
        local sub
        if m.reason == "pending" then sub = score_text(snap)
        elseif m.reason == "draw" or m.last_winner == 0 then
            sub = string.format("draw   -   %s   -   next round in %d", score_text(snap), m.countdown)
        else
            sub = string.format("%s   -   %s   -   next round in %d", round_winner_text(snap, m), score_text(snap), m.countdown)
        end
        return { title = "ROUND OVER", sub = sub }
    elseif m.state == "match_over" then
        local title, tone
        local wt = snap.mode and snap.mode.winner_team or 0
        if wt > 0 and wt == my_team(snap) then title, tone = "VICTORY", "good"
        elseif wt > 0 then title = "TEAM " .. (Mo.TEAM_NAME[wt] or "?") .. " WINS THE MATCH"
        elseif m.last_winner ~= 0 and m.last_winner == me then title, tone = "VICTORY", "good"
        elseif m.last_winner ~= 0 then title = string.upper(Mo.nick(snap, m.last_winner)) .. " WINS THE MATCH"
        else title = "MATCH OVER" end
        local why = (m.reason == "forfeit") and "opponent forfeited   -   " or ""
        return { title = title, sub = string.format("%s%s   -   back to lobby in %d", why, score_text(snap), m.countdown),
                 tone = tone }
    elseif m.state == "paused" then
        return { title = "OPPONENT DISCONNECTED",
                 sub = string.format("waiting for them to reconnect   -   %d", m.countdown) }
    end
    return nil
end

-- --- interactive panels (hud_panel.lua) -----------------------------------------------------
-- One panel at a time, most important first:
--   conn    the connection modal (lost / notice) — menu or arena
--   pause   the MP pause (Esc): RESUME / SETTINGS / LEAVE MATCH / QUIT GAME, never pauses
--   result  the match result: REMATCH / BACK TO LOBBY
-- spec = { kind, modal, title, text, buttons = { {id, label, style, disabled} }, key, vertical }
-- `key` changes whenever the panel must be rebuilt.

Mo.CONN_LABEL = { reconnect = "RECONNECT", menu = "BACK TO MENU", ok = "OK" }
Mo.REMATCH_HOLD_S = 16     -- matches the Director's rematch_hold_s (+1)

function Mo.panel(T, snap, now, ctx)
    local c = snap.conn
    if c and (c.state == "lost" or c.state == "notice") and c.seq ~= T.dismissed_seq then
        local btns = {}
        for _, a in ipairs(c.actions) do
            btns[#btns + 1] = { id = "conn:" .. a, label = Mo.CONN_LABEL[a] or string.upper(a),
                                style = (#btns == 0) and "primary" or "normal" }
        end
        if #btns == 0 then btns[1] = { id = "conn:ok", label = "OK", style = "primary" } end
        return { kind = "conn", modal = true, title = c.title or "CONNECTION LOST", text = c.text or "",
                 buttons = btns, key = "conn:" .. tostring(c.seq), tone = (c.state == "lost") and "bad" or nil }
    end
    if not ctx.in_arena then T.pause_open, T.confirm = false, nil; return nil end
    if T.pause_open then
        if T.confirm and now - T.confirm_at >= Mo.CONFIRM_S then T.confirm = nil end   -- a confirm times out
        local cf = T.confirm
        local text = "The match keeps running while this menu is open. Report a problem: open the launcher > Create bug report."
        if cf == "leave" then text = "Leave the match? In a duel your opponent wins by forfeit."
        elseif cf == "quit" then text = "Quit Half Sword? You leave the match." end
        return { kind = "pause", modal = true, title = "PAUSED", text = text, vertical = true,
                 key = "pause:" .. tostring(cf),
                 buttons = {
                     { id = "pause:resume", label = "RESUME", style = "primary" },
                     { id = "pause:settings", label = "SETTINGS" },
                     { id = "pause:leave", label = (cf == "leave") and "CONFIRM: LEAVE MATCH" or "LEAVE MATCH", style = "danger" },
                     { id = "pause:quit", label = (cf == "quit") and "CONFIRM: QUIT GAME" or "QUIT GAME", style = "danger" },
                 } }
    end
    local m = snap.match
    if m and m.state == "match_over" and T.mp.active and not T.tab then
        local key = "result:" .. tostring(m.round) .. ":" .. tostring(m.last_winner)
        if T.result_key ~= key then T.result_key, T.result_choice, T.result_at = key, nil, nil end
        if T.result_choice == "menu" then return nil end
        local waiting = T.result_choice == "rematch"
        local label = waiting and (snap.is_admin and "REMATCH: STARTING WHEN READY" or "REMATCH: READY") or "REMATCH"
        return { kind = "result", modal = false, key = key .. ":" .. tostring(T.result_choice),
                 buttons = { { id = "result:rematch", label = label, style = "primary", disabled = waiting },
                             { id = "result:menu", label = "BACK TO LOBBY" } } }
    end
    return nil
end

-- REMATCH pressed and the Director still holds us in the arena (lobby phase).
function Mo.rematch_waiting(T, now)
    return T.result_choice == "rematch" and T.result_at ~= nil and now - T.result_at < Mo.REMATCH_HOLD_S
end

-- A panel button was clicked. Returns the Director want for the ui_request bus key
-- (or nil) and an action for main.lua ("quit", "settings", nil).
function Mo.click(T, id, now)
    local kind, what = id:match("^(%a+):(%a+)$")
    if kind == "conn" then
        T.dismissed_seq = T.conn and T.conn.seq or T.dismissed_seq
        if what == "reconnect" then return "reconnect" end
        if what == "menu" then return "dismiss" end
        return "ok"
    elseif kind == "pause" then
        if what == "resume" then T.pause_open, T.confirm = false, nil; return nil end
        if what == "settings" then T.pause_open, T.confirm = false, nil; return nil, "settings" end
        if what == "leave" or what == "quit" then
            if T.confirm == what and now - T.confirm_at < Mo.CONFIRM_S then
                T.pause_open, T.confirm = false, nil
                return "leave", (what == "quit") and "quit" or nil
            end
            T.confirm, T.confirm_at = what, now
            return nil
        end
    elseif kind == "result" then
        if what == "rematch" and T.result_choice ~= "rematch" then
            T.result_choice, T.result_at = "rematch", now
            return "rematch"
        end
        if what == "menu" then T.result_choice = "menu"; return "menu" end
    end
    return nil
end

-- --- the full model -------------------------------------------------------------------------

-- ctx = { in_arena, menu_open, centre_enabled }
function Mo.build(T, snap, now, ctx)
    local out = { visible = false, why = "" }
    if not ctx.in_arena then out.why = "not in an arena"; return out end
    if ctx.menu_open then out.why = "menu open"; return out end
    local m = snap.match
    local running = m and SHOWN[m.state] and T.mp.active
    local reconnecting = T.conn and T.conn.state == "reconnecting"
    local rematch = m and m.state == "lobby" and Mo.rematch_waiting(T, now)
    if not running and not reconnecting and not rematch and not T.link_lost_at and not snap.rejected then
        out.why = T.mp.active and "no match running" or "no MP session"
        return out
    end
    out.visible = true
    local me = my_id(snap)

    -- top-centre: round, best-of, timer + score strip
    if m then
        local rnd = (m.state == "countdown") and (m.round + 1) or math.max(1, m.round)
        local title = string.format("ROUND %d", rnd)
        local md = snap.mode
        if md and md.id == "koth" and m.state == "live" then
            title = title .. "  /  HILL: " .. (Mo.hill_hint(md.zone, ctx.me_pos, ctx.me_yaw) or "") ..
                ((hill_holder(snap) and ("  -  " .. hill_holder(snap))) or "")
        elseif md and md.id ~= "duel" and md.id ~= "ffa" then
            title = title .. "  /  " .. string.upper(md.label or md.id)
        elseif m.best_of and m.best_of > 0 then
            title = title .. string.format("  /  BEST OF %d", m.best_of)
        end
        local timer
        if m.state == "live" and md and md.round_left_ms then
            timer = (md.sudden_death and "SUDDEN DEATH " or "") .. mmss(md.round_left_ms / 1000)
        elseif m.state == "live" and m.round_left then timer = mmss(m.round_left)
        elseif m.state == "live" and T.live_since then timer = mmss(now - T.live_since)
        elseif m.state == "countdown" and #m.waiting > 0 then timer = "LOADING"
        elseif m.state == "paused" then timer = "PAUSED " .. mmss(m.countdown)
        elseif m.state == "match_over" then timer = "END"
        else timer = mmss(m.countdown) end
        local cells = {}
        local tm = teams_of(snap)
        if tm then
            -- one cell per team: round wins (and this round's points in King of the hill / deathmatch)
            local mine = my_team(snap)
            for t = 1, tm.teams do
                local sc = round_score(tm, tm.team_score[t])
                cells[#cells + 1] = { id = -t, text = string.format("%s  %d%s", Mo.TEAM_NAME[t], tm.team_wins[t] or 0,
                    sc and ("  (" .. sc .. ")") or ""), me = (t == mine),
                    dead = (m.state == "live" and (tm.team_alive[t] or 0) == 0 and tm.id ~= "deathmatch") }
            end
        else
            for _, id in ipairs(m.order) do
                local r = md and md.rows[id]
                local sc = md and r and round_score(md, r.score)
                cells[#cells + 1] = { id = id, text = string.format("%s  %d%s", Mo.nick(snap, id), m.wins[id] or 0,
                                                                    sc and ("  (" .. sc .. ")") or ""),
                                      me = (id == me), dead = (m.state == "live" and m.alive[id] == false) }
            end
        end
        out.top = { title = title, timer = timer, cells = cells }
    end

    -- centre message (gated: HSMPMatch still owns the banner until the hand-off)
    if ctx.centre_enabled and not T.tab and not ctx.modal then out.centre = Mo.centre(T, snap, now) end

    -- local vitals
    -- my own `vitals` record (CON / BODY / BLEED: the same reader and
    -- quantisation as the opponents' records)
    local vo = snap.vown
    if vo and vo.hp then
        local hp = vo.hp
        local dead = (m and m.alive[me] == false and m.state == "live") or hp <= 0 or vo.dead or false
        out.me = { hp = math.max(0, hp), st = vo.st, dead = dead and true or false,
                   body = vo.body, con = vo.con, bleeding = vo.bleeding or false, severed = vo.severed or false }
    end

    -- opponents (roster order) from their `peer_vitals` records
    out.opps = {}
    if m then
        for _, id in ipairs(m.order) do
            if id ~= me then
                local r = snap.vremote and snap.vremote[id]
                local hp = r and r.hp
                local st = r and r.st
                local dead = (r and r.dead) or (m.state == "live" and m.alive[id] == false)
                local down = r and r.down
                out.opps[#out.opps + 1] = { id = id, name = Mo.nick(snap, id), hp = hp, st = st,
                                            dead = dead and true or false, down = (not dead) and down and true or false,
                                            body = r and r.body, con = r and r.con, bleeding = r and r.bleeding or false,
                                            severed = r and r.severed or false }
            end
        end
    end

    -- kill feed (newest at the top), with fade
    out.feed = {}
    for i = #T.feed, 1, -1 do
        local e = T.feed[i]
        local age = now - e.at
        if age < Mo.FEED_TTL_S + Mo.FEED_FADE_S then
            local a = 1
            if age > Mo.FEED_TTL_S then a = 1 - (age - Mo.FEED_TTL_S) / Mo.FEED_FADE_S end
            out.feed[#out.feed + 1] = { text = e.text, alpha = a, mine = e.mine, notice = e.notice, host = e.host }
        end
    end

    out.net = Mo.net(T, snap, now)

    -- HUD switches (docs/development/subsystems/menu-ui.md): hud=false hides vitals, opponents,
    -- kill feed and net; the round / phase banner, the centre message, server
    -- notices and the panels stay (they carry critical state). hud_killfeed
    -- hides the kill lines (notices stay); hud_net = off | bad (only when the
    -- link is not good) | always.
    local p = T.prefs or Mo.PREFS_DEFAULT
    local feed_on = p.hud ~= false and p.killfeed ~= false
    if not feed_on then
        local keep = {}
        for _, f in ipairs(out.feed) do if f.notice or f.host then keep[#keep + 1] = f end end
        out.feed = keep
    end
    if p.hud == false then
        out.me, out.opps, out.net = nil, {}, nil
    elseif p.net == "off" then
        out.net = nil
    elseif p.net == "bad" and out.net and (out.net.level == "good" or out.net.level == "ok" or out.net.level == "none") then
        out.net = nil
    end

    -- scoreboard (hold Tab)
    if T.tab and m then
        local rows = {}
        local ids = {}
        for _, id in ipairs(m.order) do ids[#ids + 1] = id end
        local md = snap.mode
        local function row_of(id) return md and md.rows[id] or nil end
        table.sort(ids, function(a, b)
            local ra, rb = row_of(a), row_of(b)
            local ta, tb = ra and ra.team or 0, rb and rb.team or 0
            if ta ~= tb then return ta < tb end
            local wa, wb = m.wins[a] or 0, m.wins[b] or 0
            if wa ~= wb then return wa > wb end
            local ka, kb = ra and ra.kills or 0, rb and rb.kills or 0
            if ka ~= kb then return ka > kb end
            return a < b
        end)
        local waiting = {}
        for _, id in ipairs(m.waiting) do waiting[id] = true end
        for i, id in ipairs(ids) do
            local ping = snap.sc and snap.sc.ping[id]
            if id == me and snap.metrics and snap.metrics.rtt then ping = snap.metrics.rtt end
            local status
            local r = row_of(id)
            if waiting[id] then status = "LOADING"
            elseif r and r.respawning then status = "RESPAWN"
            elseif m.alive[id] == false then status = "DEAD"
            else status = "ALIVE" end
            local tag = (r and r.team > 0) and ("[" .. (Mo.TEAM_NAME[r.team] or "?") .. "] ") or ""
            rows[#rows + 1] = { rank = tostring(i), nick = tag .. Mo.nick(snap, id) .. ((id == me) and "  (you)" or ""),
                                wins = tostring(m.wins[id] or 0),
                                kd = r and string.format("%d / %d", r.kills, r.deaths) or "-",
                                ping = ping and string.format("%d ms", math.floor(ping + 0.5)) or "-",
                                status = status, me = (id == me) }
        end
        local arena = Mo.arena_label(m.arena)
        local rnd = (m.state == "countdown") and (m.round + 1) or math.max(1, m.round)
        out.board = {
            title = string.format("SCOREBOARD  -  %sROUND %d%s", (m.best_of > 0) and string.format("BEST OF %d  -  ", m.best_of) or "",
                                  rnd, arena ~= "" and ("  -  " .. string.upper(arena)) or ""),
            rows = rows,
            foot = "release TAB to close",
        }
    end
    return out
end

return Mo
