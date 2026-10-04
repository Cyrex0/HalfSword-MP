-- The owner's passport body on its stand-ins (HSMPCombat; loaded by main.lua).
--
-- A stand-in is a pooled Willie and keeps the body of the foe it was: Height
-- Rate, Muscle Rate, "Mass Scale (Set in BP)" and bone masses up to 2.6x the
-- owner's (measured with HSMPParity `kit`, combat-parity.md). The damage the
-- victim takes is its own replay, but the normal impulse the attacker's blade
-- gets from the stand-in is not: Collision Hit's Hit Velocity is the larger of
-- the weapon's speed and that impulse, so a heavier stand-in made heavier blows.
--
-- Owner side: every few seconds the own pawn's body is read into the `body`
-- record (game slot `body`; a new version only when it changed). The sidecar
-- sends it to a server that negotiated caps::BODY; the other players' sidecars
-- write it into the per-peer slot `peer_body`.
--
-- Stand-in side: the rates and the BP mass scale are written, and every bone
-- whose mass differs from the owner's by more than MASS_TOL gets the mass scale
-- that gives it the owner's mass (SetMassScale on the stand-in's Mesh). The
-- geometry (Character Scale, the stand-in's height) is carried but not applied:
-- HSMPAvatars measures the stand-in's bone offsets once per drive, and a mesh
-- rescaled under it would stretch every joint (APPLY_SCALE stays false until
-- Avatars re-measures on a scale change).
--
-- No UObject is kept here: everything is keyed by actor FName and redone from
-- the objects main.lua passes in, on the game thread, inside its world guard.

local B = {
    -- The Willie's simulated bodies (codec v2 bones).
    BONES = { "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head",
              "clavicle_l", "clavicle_r", "upperarm_l", "upperarm_r", "lowerarm_l", "lowerarm_r", "hand_l", "hand_r",
              "thigh_l", "thigh_r", "calf_l", "calf_r", "foot_l", "foot_r" },
    MAX_BONES = 24,
    PUBLISH_TICKS = 60,     -- own body read every 2 s (33 ms ticks)
    APPLY_TICKS = 30,       -- stand-ins checked once a second
    MASS_TOL = 0.01,        -- relative mass error left alone
    SCALE_MIN = 0.01, SCALE_MAX = 16.0,
    APPLY_SCALE = false,
    version = 0, last_key = nil, written = 0,
    applied = {},           -- stand-in FName -> { ver = peer record version, n = bones set, at = tick }
    fights = {},            -- stand-in FName .. bone -> times the mass had to be set again
}

local function num(x) return tonumber(x) end

local function fname(s) return FName(s) end

-- The body of Willie `w` (its Mesh `mesh`) as a `body` record (no version), or
-- nil when nothing is readable. Property names keep their spaces.
function B.read(w, mesh)
    local r = { rows = {} }
    pcall(function() r.height_rate = num(w["Height Rate"]) end)
    pcall(function() r.muscle_rate = num(w["Muscle Rate"]) end)
    pcall(function() r.mass_scale_bp = num(w["Mass Scale (Set in BP)"]) end)
    pcall(function()
        local c = w["Character Scale (Set in BP)"]
        r.char_scale = { num(c.X), num(c.Y), num(c.Z) }
    end)
    if not (r.height_rate and r.muscle_rate and r.mass_scale_bp) then return nil end
    local cs = r.char_scale
    if not (cs and cs[1] and cs[2] and cs[3] and cs[1] > 0 and cs[2] > 0 and cs[3] > 0) then r.char_scale = { 1, 1, 1 } end
    for _, b in ipairs(B.BONES) do
        local m, s
        pcall(function() m = num(mesh:GetBoneMass(fname(b), true)) end)
        pcall(function() s = num(mesh:GetMassScale(fname(b))) end)
        if m and s and m > 0 and m <= 500 and s > 0 and s <= B.SCALE_MAX and #r.rows < B.MAX_BONES then
            r.rows[#r.rows + 1] = { bone = b, mass = m, mass_scale = s }
        end
    end
    if #r.rows == 0 then return nil end
    -- the record's bounds (schema/loadout.rs check_body)
    r.height_rate = math.max(0, math.min(4, r.height_rate))
    r.muscle_rate = math.max(0, math.min(4, r.muscle_rate))
    if not (r.mass_scale_bp > 0 and r.mass_scale_bp <= B.SCALE_MAX) then return nil end
    return r
end

-- A change key: 3 significant digits per value (float noise is not a new body).
function B.key(r)
    local p = { string.format("%.3g|%.3g|%.3g|%.3g|%.3g|%.3g", r.height_rate, r.muscle_rate, r.mass_scale_bp,
        r.char_scale[1], r.char_scale[2], r.char_scale[3]) }
    for _, x in ipairs(r.rows) do p[#p + 1] = string.format("%s=%.3g/%.3g", x.bone, x.mass, x.mass_scale) end
    return table.concat(p, ",")
end

-- Owner side: write the own body into the `body` slot when it changed.
-- `put(slot, rec)` is IPC.put. Returns the version written, or nil.
function B.publish(me, mesh, put, log)
    local r = B.read(me, mesh)
    if not r then return nil end
    local k = B.key(r)
    if k == B.last_key then return nil end
    -- Versions survive a game restart (the sidecar and server keep the newest).
    B.version = math.max(B.version + 1, (os.time() - 1700000000) * 16)
    r.version = B.version
    local ok, err = put("body", r)
    if not ok then
        if log then log("body: own passport body NOT written (%s)", tostring(err)) end
        return nil
    end
    B.last_key = k
    B.written = B.written + 1
    if log then
        log("body: own passport body v=%d height=%.3f muscle=%.3f mass_scale_bp=%.3f bones=%d pelvis=%.2f kg",
            r.version, r.height_rate, r.muscle_rate, r.mass_scale_bp, #r.rows, r.rows[1].mass)
    end
    return r.version
end

-- Stand-in side: give stand-in `w` (FName `nm`, Mesh `mesh`) the owner's body
-- record `rec` (a `peer_body` table). Returns the number of bones whose mass
-- scale was set.
function B.apply(nm, w, mesh, rec)
    if type(rec) ~= "table" or type(rec.rows) ~= "table" then return 0 end
    for k, f in pairs({ ["Height Rate"] = "height_rate", ["Muscle Rate"] = "muscle_rate",
                        ["Mass Scale (Set in BP)"] = "mass_scale_bp" }) do
        local want = num(rec[f])
        if want then
            pcall(function()
                local cur = num(w[k])
                if cur == nil or math.abs(cur - want) > 1e-3 then w[k] = want end
            end)
        end
    end
    local st = B.applied[nm]
    local n = 0
    for _, row in ipairs(rec.rows) do
        local bone, want = row.bone, num(row.mass)
        if type(bone) == "string" and bone ~= "" and want and want > 0 then
            local m, s
            pcall(function() m = num(mesh:GetBoneMass(fname(bone), true)) end)
            pcall(function() s = num(mesh:GetMassScale(fname(bone))) end)
            if m and s and m > 0 and s > 0 and math.abs(m - want) > B.MASS_TOL * want then
                local ns = math.max(B.SCALE_MIN, math.min(B.SCALE_MAX, s * want / m))
                if pcall(function() mesh:SetMassScale(fname(bone), ns) end) then
                    n = n + 1
                    -- set before for this record: something in the game put it back
                    if st and st.ver == rec.version then
                        local fk = nm .. "|" .. bone
                        B.fights[fk] = (B.fights[fk] or 0) + 1
                    end
                end
            end
        end
    end
    B.applied[nm] = { ver = rec.version, n = n }
    return n
end

-- Times the game undid a mass we set (the 5 s log line).
function B.fight_count()
    local n = 0
    for _, c in pairs(B.fights) do n = n + c end
    return n
end

-- World drop: stand-in names are reused by the next world.
function B.drop()
    B.applied, B.fights = {}, {}
end

return B
