-- Spawn proof uses the streams actually sampled/applied for this life. A
-- visible actor or an input slot populated by a previous life is not proof.
-- Pure Lua: callers supply current native readbacks; no UObject is retained.
local R = { POSE_MS = 250, VITALS_MS = 2500, SETTLE_MS = 150, POS_UU = 5, ROT_DEG = 10 }
local function finite(n) return type(n) == "number" and n == n and n ~= math.huge and n ~= -math.huge end
local function scoped(record, context)
    return type(record) == "table" and type(context) == "table"
        and (context.match_id or 0) ~= 0 and (context.round or 0) > 0 and (context.life or 0) > 0
        and record.match_id == context.match_id and record.round == context.round and record.life == context.life
end
R.scoped = scoped
local function fresh(now, stamp, limit)
    return type(stamp) == "number" and stamp == stamp and stamp <= now and now - stamp <= limit
end
local function vitals(record, context)
    if not scoped(record, context) then return false, "vitals life" end
    local hp = type(record.v) == "table" and record.v[1]
    local flags = math.tointeger(tonumber(record.flags))
    if not flags or flags & 1 ~= 0 or type(hp) ~= "number" or hp <= 0 or hp >= 65535 then
        return false, "vitals unavailable/dead"
    end
    return true
end
R.vitals = vitals
function R.physical(playback, remote, world, now)
    if type(world) ~= "string" or world == "" or not playback or playback.settle_world ~= world then
        return false, "physical world unavailable/changed"
    end
    if not fresh(now, playback.settle_sample_ms, R.POSE_MS) then return false, "physical sample unavailable/stale/future" end
    local seq = math.tointeger(tonumber(playback.settle_source_seq))
    local cut = math.tointeger(tonumber(remote.source and remote.source.cut))
    if not seq or seq < 0 or not finite(playback.settle_source_ts) or playback.settle_source_ts <= 0
        or not cut or playback.settle_cut ~= cut then return false, "physical integrated aim unavailable/cut" end
    if playback.settle_ready ~= true then return false, "physical " .. tostring(playback.settle_reason or "unavailable") end
    if playback.settle_count ~= 6 then return false, "physical limbs incomplete" end
    if not finite(playback.settle_stable_ms) or playback.settle_stable_ms < R.SETTLE_MS then return false, "physical stabilizing" end
    if not finite(playback.settle_pos_uu) or playback.settle_pos_uu < 0 or playback.settle_pos_uu > R.POS_UU then
        return false, "physical limb position"
    end
    if not finite(playback.settle_rot_deg) or playback.settle_rot_deg < 0 or playback.settle_rot_deg > R.ROT_DEG then
        return false, "physical limb rotation"
    end
    return true
end
function R.new()
    local self = { counters = {}, qualified = {} }
    function self:reset() self.counters, self.qualified, self.key = {}, {}, nil end
    function self:observe(key, value, now)
        value = math.tointeger(tonumber(value))
        if not value or value < 0 then return false end
        local prior = self.counters[key]
        if not prior then self.counters[key] = { value = value }; return false end
        local delta = (value - prior.value) % 0x100000000
        if delta > 0 and delta < 0x80000000 then
            prior.value, prior.at = value, now
        end
        return prior.at ~= nil, prior.at
    end
    function self:check(input)
        local own, now = input.own, input.now_ms
        if type(own) ~= "table" or type(now) ~= "number" or now ~= now then return false, "spawn context unavailable" end
        local key = tostring(input.world or "") .. "|" .. tostring(own.match_id) .. ":" .. tostring(own.round)
            .. ":" .. tostring(own.life) .. "@" .. tostring(own.pawn)
        if self.key ~= key then self:reset(); self.key = key end
        -- Seed all counters together. Only a subsequent sample proves that a
        -- retained slot is still being written; no fixed wait releases it.
        local pose_ok, pose_at
        if scoped(input.pose, own) then
            pose_ok, pose_at = self:observe("pose", input.pose.tick, now)
        end
        local own_vok, own_vat
        if scoped(input.vitals, own) then own_vok, own_vat = self:observe("vitals", input.vitals.seq, now) end
        local remote_seen = {}
        local qualify = {}
        for _, remote in ipairs(input.remotes or {}) do
            if scoped(remote.vitals, remote) then
                local advanced, at = self:observe("peer:" .. tostring(remote.peer) .. ":" .. tostring(remote.life), remote.vitals.seq, now)
                remote_seen[remote.peer] = { advanced, at }
            end
        end
        if not scoped(input.root, own) then return false, "own root life" end
        if type(input.root.ts) ~= "number" or (now % 0x100000000 - input.root.ts) % 0x100000000 > R.POSE_MS then
            return false, "own root stale/future"
        end
        -- sample_status().pose is captured only after an actual successful
        -- native/Lua pose write. The local_pose slot header omits its encoded
        -- life context, so use this exact successful-write context instead.
        if not scoped(input.pose, own) then return false, "own pose proof unavailable/life" end
        if not fresh(now, input.pose.ts, R.POSE_MS) or not pose_ok or not fresh(now, pose_at, R.POSE_MS) then
            return false, "own pose not sampling"
        end
        local ok, why = vitals(input.vitals, own)
        if not ok then return false, "own " .. why end
        if not own_vok or not fresh(now, own_vat, R.VITALS_MS) then return false, "own vitals not sampling" end
        for _, remote in ipairs(input.remotes or {}) do
            local prefix = "peer " .. tostring(remote.peer) .. " "
            local source = remote.source
            if not scoped(source, remote) or source.has_context ~= true or source.peer_id ~= remote.peer then
                return false, prefix .. "pose source unavailable/life"
            end
            -- PeerPlay's publication sequence and evaluated clock continue
            -- advancing while extrapolating a stopped sender. Its source age
            -- measures the newest actual physical frame instead.
            if (source.mode ~= "interp" and source.mode ~= "extrap") or type(source.age) ~= "number"
                or source.age ~= source.age or math.abs(source.age) > R.POSE_MS then
                return false, prefix .. "pose source stale/held"
            end
            local playback = remote.playback
            if not scoped(playback, remote) or not remote.pawn or remote.pawn == "" or playback.pawn ~= remote.pawn then
                return false, prefix .. "displayed pawn/life"
            end
            if not fresh(now, playback.local_ms, R.POSE_MS) then return false, prefix .. "playback stale/future" end
            ok, why = vitals(remote.vitals, remote)
            if not ok then return false, prefix .. why end
            local seen = remote_seen[remote.peer]
            if not seen or not seen[1] or not fresh(now, seen[2], R.VITALS_MS) then return false, prefix .. "vitals not sampling" end
            if not remote.native or remote.native.alive ~= true then return false, prefix .. "native pawn unavailable/dead" end
            if remote.native.collision ~= true then return false, prefix .. "native collision unavailable/off" end
            local cut = math.tointeger(tonumber(source.cut))
            local token = tostring(input.native_world) .. "|" .. tostring(remote.peer) .. "|" .. tostring(remote.match_id)
                .. ":" .. tostring(remote.round) .. ":" .. tostring(remote.life) .. "@" .. remote.pawn .. ":" .. tostring(cut)
            -- Only a successful first release/handover qualifies a life. Ready
            -- and Countdown keep checking current physical samples. Once
            -- released, pause/reconnect recheck streams without demanding that
            -- an injured same-life limb recover its spawn alignment.
            if self.qualified[remote.peer] ~= token then self.qualified[remote.peer] = nil end
            if not (input.qualify_settle == true and cut and self.qualified[remote.peer] == token) then
                ok, why = R.physical(playback, remote, input.native_world, now)
                if not ok then return false, prefix .. why end
            end
            qualify[#qualify + 1] = { peer = remote.peer, token = token }
        end
        if input.qualify_settle == true then for _, entry in ipairs(qualify) do self.qualified[entry.peer] = entry.token end end
        return true, "current life pose/vitals/collision/physical"
    end
    return self
end
return R
