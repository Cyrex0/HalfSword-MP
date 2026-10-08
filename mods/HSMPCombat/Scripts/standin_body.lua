-- The owner's passport body on its stand-ins (HSMPCombat; loaded by main.lua).
--
-- A stand-in is a pooled Willie and keeps the body of the foe it was: Height
-- Rate, Muscle Rate, "Mass Scale (Set in BP)" and bone masses up to 2.6x the
-- owner's (measured with HSMPParity `kit`, combat-parity.md). The damage the
-- victim takes is its own replay, but the normal impulse the attacker's blade
-- gets from the stand-in is not: Collision Hit's Hit Velocity is the larger of
-- the weapon's speed and that impulse, so a heavier stand-in made heavier blows.
--
-- Owner side: the verified pawn body is read into `body2` immediately for
-- a new native life, then periodically on change. The sidecar negotiates
-- caps::BODY2 and preserves source generation in per-peer `peer_body2`.
--
-- Stand-in side: the rates and the BP mass scale are written, and every bone
-- whose mass differs from the owner's by more than MASS_TOL gets the mass scale
-- that gives it the owner's mass (SetMassScale on the stand-in's Mesh). The
-- geometry is carried for HSMPAvatars, which applies it through a clean
-- physics reset and re-measures its servo geometry. Combat never resizes a
-- driven mesh underneath the avatar driver's cached joint offsets.
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

-- Native dismemberment reports the severed root. Contacts with any child of
-- that root are equally absent even though a pooled stand-in retains its
-- reversible physics bodies under HideBoneByName(PBO_None).
local PARENT = {
    spine_01 = "pelvis", spine_02 = "spine_01", spine_03 = "spine_02", spine_04 = "spine_03", spine_05 = "spine_04",
    neck_01 = "spine_05", neck_02 = "neck_01", head = "neck_02",
    clavicle_l = "spine_05", upperarm_l = "clavicle_l", lowerarm_l = "upperarm_l", hand_l = "lowerarm_l",
    clavicle_r = "spine_05", upperarm_r = "clavicle_r", lowerarm_r = "upperarm_r", hand_r = "lowerarm_r",
    thigh_l = "pelvis", calf_l = "thigh_l", foot_l = "calf_l",
    thigh_r = "pelvis", calf_r = "thigh_r", foot_r = "calf_r",
}
function B.severed(bone, dism)
    if type(bone) ~= "string" or type(dism) ~= "table" then return false end
    local missing = {}
    for _, root in ipairs(dism) do
        if type(root) == "string" then missing[string.lower(root)] = true end
    end
    bone = string.lower(bone)
    while bone do
        if missing[bone] then return true end
        bone = PARENT[bone]
    end
    return false
end

local function num(x) return tonumber(x) end

local function fname(s) return FName(s) end

-- Dev diagnostic only. Caller owns world/identity guards and the avatar
-- driver's physics-off/re-pose window. Never invoked by body replication.
B.HEIGHT_FIELD = "Height_21_0EB204DF4978B92AD0ED188FD32EEC7B"
local function scale_copy(c)
    if not c then return nil end
    local r = { X = num(c.X), Y = num(c.Y), Z = num(c.Z) }
    for _, k in ipairs({"X","Y","Z"}) do
        if not r[k] or r[k] ~= r[k] or r[k] <= 0 or r[k] > 16 then return nil end
    end
    return r
end
function B.height_snapshot(w, mesh)
    local ok, s = pcall(function()
        return { height = num(w["Character Passport"][B.HEIGHT_FIELD]),
            actor_scale = scale_copy(w:GetActorScale3D()), mesh_scale = scale_copy(mesh:K2_GetComponentScale()) }
    end)
    if not ok or not s.height or s.height ~= s.height or not s.actor_scale or not s.mesh_scale then
        return nil, "native height/scale snapshot unavailable"
    end
    return s
end
function B.height_restore(w, mesh, s)
    -- Each write is attempted even if an earlier one fails; caller retains
    -- the snapshot until ALL writes succeed, rather than losing restoration.
    local a = pcall(function() w["Character Passport"][B.HEIGHT_FIELD] = s.height end)
    local b = pcall(function() w:SetActorScale3D(s.actor_scale) end)
    local c = pcall(function() mesh:SetWorldScale3D(s.mesh_scale) end)
    return a and b and c
end
function B.height_probe(w, mesh, height, s)
    if type(height) ~= "number" or height ~= height or height < 0 or height > 1 then
        return false, "diagnostic height must be 0..1"
    end
    local ok, err = pcall(function()
        w["Character Passport"][B.HEIGHT_FIELD] = height
        w["Set Character Height"](w) -- native bytecode: SetActorScale3D only
        mesh:SetWorldScale3D(s.mesh_scale)
    end)
    return ok, err
end

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
    -- The BP field can precede a mesh rebuild. Replicate the geometry that
    -- actually collides and renders on the owner, including inherited scale.
    pcall(function()
        local c = mesh:K2_GetComponentScale()
        r.char_scale = { num(c.X), num(c.Y), num(c.Z) }
    end)
    if not (r.height_rate and r.muscle_rate and r.mass_scale_bp) then return nil end
    local cs = r.char_scale
    if not (cs and cs[1] and cs[2] and cs[3] and cs[1] > 0 and cs[2] > 0 and cs[3] > 0) then r.char_scale = { 1, 1, 1 } end
    for _, b in ipairs(B.BONES) do
        local m, s
        -- BodyInstance mass already reflects SetMassScale. bScaleMass=true
        -- applies the scale again; feeding it back makes corrections oscillate.
        pcall(function() m = num(mesh:GetBoneMass(fname(b), false)) end)
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
function B.context_matches(rec, ctx)
    return type(rec) == "table" and type(ctx) == "table" and rec.match_id ~= nil and rec.match_id ~= 0
        and rec.match_id == ctx.match_id and rec.round == ctx.round and rec.life ~= nil and rec.life > 0
        and rec.life == ctx.life and type(rec.pawn) == "string" and rec.pawn ~= ""
end

-- Owner side: write the own body into the `body` slot when it changed.
-- `put(slot, rec)` is IPC.put. Returns the version written, or nil.
function B.publish(me, mesh, put, log, ctx)
    if type(ctx) ~= "table" or not ctx.match_id or ctx.match_id == 0 or not ctx.round or ctx.round < 1
        or not ctx.life or ctx.life < 1 then return nil end
    local ok_name, source_name = pcall(function() return me:GetFName():ToString() end)
    if not ok_name or source_name ~= ctx.pawn then return nil end
    local r = B.read(me, mesh)
    if not r then return nil end
    local native = B.height_snapshot(me, mesh)
    if not native or native.height < 0 or native.height > 1 then return nil end
    r.match_id, r.round, r.life, r.pawn = ctx.match_id, ctx.round, ctx.life, source_name
    r.native_height = native.height
    r.actor_scale = { native.actor_scale.X, native.actor_scale.Y, native.actor_scale.Z }
    local k = string.format("%s|%d|%d|%s|%.6g|%.6g/%.6g/%.6g|", tostring(r.match_id), r.round, r.life,
        r.pawn, r.native_height, r.actor_scale[1], r.actor_scale[2], r.actor_scale[3]) .. B.key(r)
    if k == B.last_key then return nil end
    -- Versions survive a game restart (the sidecar and server keep the newest).
    B.version = math.max(B.version + 1, (os.time() - 1700000000) * 16)
    r.version = B.version
    local ok, err = put("body2", r)
    if not ok then
        if log then log("body: own passport body NOT written (%s)", tostring(err)) end
        return nil
    end
    B.last_key = k
    B.written = B.written + 1
    if log then
        log("body2: source=%s match=%s round=%d life=%d v=%d native_height=%.4f actor_scale=%.4f/%.4f/%.4f mesh_scale=%.4f/%.4f/%.4f height_rate=%.3f muscle=%.3f mass_scale_bp=%.3f bones=%d pelvis=%.2f kg",
            r.pawn,tostring(r.match_id),r.round,r.life,r.version,r.native_height,
            r.actor_scale[1],r.actor_scale[2],r.actor_scale[3],r.char_scale[1],r.char_scale[2],r.char_scale[3],
            r.height_rate,r.muscle_rate,r.mass_scale_bp,#r.rows,r.rows[1].mass)
    end
    return r.version
end

-- Stand-in side: give stand-in `w` (FName `nm`, Mesh `mesh`) the owner's body
-- record `rec` (a `peer_body` table). Returns the number of bones whose mass
-- scale was set.
function B.apply(nm, w, mesh, rec, ctx)
    if not B.context_matches(rec, ctx) then return 0 end
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
            pcall(function() m = num(mesh:GetBoneMass(fname(bone), false)) end)
            pcall(function() s = num(mesh:GetMassScale(fname(bone))) end)
            if m and s and m > 0 and s > 0 and math.abs(m - want) > B.MASS_TOL * want then
                local ns = math.max(B.SCALE_MIN, math.min(B.SCALE_MAX, s * want / m))
                local ok, result = pcall(function() return mesh:SetMassScale(fname(bone), ns) end)
                if ok and result ~= false then
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
