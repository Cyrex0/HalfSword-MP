-- kit.lua — HSMP classes / gear selection: applies the server-validated kit.
--
-- Inputs (written by hsmp-sidecar, see server/src/loadout_client.rs):
--   per-peer slot "peer_kit"   kit_verdict record { class, r, l, cos[4], rev, seq (ack),
--                              verdict, reason, rows = { {id} } } (schema loadout.rs)
--   slot "kit_rules"           kit_rules record { mode, budget, rev }
--   link record (slot `link`)  our own peer id (shared/hsmp_session.lua)
--
-- Own pawn: the kit is applied after every spawn (new pawn), every new round
-- and every arena load; a kit/rules change during a live round waits for the
-- next round. Each apply is verified 3 s later (all kit armour classes worn)
-- and retried up to 3 times.
--
-- Stand-ins (puppets): main.lua asks effective_remote() for the loadout to
-- show. The owner's replicated appearance (peer_loadout record) is the truth for
-- the exact look (damage, colours) — but under CLASSES/CUSTOM rules only the
-- armour pieces that belong to the validated kit are shown. With no
-- appearance yet, the kit itself is shown.
--
-- Every UE call here runs on the game thread (called from main.lua's
-- LoopAsync, which the thread shim maps to LoopInGameThreadWithDelay).

local Kit = { _hid_note = {} }
local api, Cat
local function Log(fmt, ...) if api then api.Log("[kit] " .. fmt, ...) end end

local function load_catalog()
    local ok, c = pcall(require, "hsmp_catalog")
    if ok and type(c) == "table" then return c end
    -- Fallback: next to this file.
    local src = debug.getinfo(1, "S").source or ""
    local dir = src:gsub("^@", ""):gsub("\\", "/"):match("^(.*)/[^/]*$")
    if dir then
        local ok2, c2 = pcall(dofile, dir .. "/hsmp_catalog.lua")
        if ok2 and type(c2) == "table" then return c2 end
    end
    return nil
end

function Kit.init(a)
    api = a
    Cat = load_catalog()
    if Cat then
        local n = 0; for _ in pairs(Cat.items) do n = n + 1 end
        Log("catalogue loaded: %d items, %d classes", n, #Cat.classes)
    else
        Log("ERROR: hsmp_catalog.lua not found — kits disabled")
    end
end

-- --- shared memory (schema loadout.rs records) ---------------------------------

local function ipc() return rawget(_G, "HSMP_IPC") end

-- The typed session / link view (shared/hsmp_session.lua), loaded once (api.hsmp_session in tests).
local HS_mod
local function HS()
    if api and api.hsmp_session then return api.hsmp_session end
    if HS_mod ~= nil then return HS_mod or nil end
    local ok, m = pcall(require, "hsmp_session")
    if not (ok and type(m) == "table") then
        local src = (debug.getinfo(1, "S").source or ""):gsub("^@", ""):gsub("\\", "/")
        local dir = src:match("^(.*)/[^/]*$") or "."
        for _, p in ipairs({ dir .. "/hsmp_session.lua", dir .. "/../../shared/hsmp_session.lua" }) do
            local ok2, m2 = pcall(dofile, p)
            if ok2 and type(m2) == "table" then m = m2; ok = true; break end
        end
    end
    HS_mod = (ok and type(m) == "table") and m or false
    return HS_mod or nil
end
Kit._HS = HS

-- Own peer id from the sidecar's link record. A missing record keeps the last good id:
-- a transient 0 would report "no peer id yet" in kit_status and stall the Director's
-- kit step.
local last_peer_id = 0
function Kit.my_peer_id()
    local hs = HS()
    local id = hs and hs.my_peer_id() or 0
    if id > 0 then last_peer_id = id end
    return last_peer_id
end

-- The server-validated kit of `pid` (per-peer slot "peer_kit", a `kit_verdict` record:
-- class, r, l, cos[4], rev, seq = acked selection, verdict, reason, rows = armour ids).
function Kit.read(pid)
    if not pid or pid == 0 then return nil end
    local I = ipc()
    local k = I and I.peer_rec and I.peer_rec("peer_kit", pid)
    if type(k) == "table" and type(k.class) == "string" and k.class ~= "" and (tonumber(k.rev) or 0) > 0 then return k end
    return nil
end

-- Armour ids of a kit record (its rows), cached per record table.
local ids_of = setmetatable({}, { __mode = "k" })
local function armor_ids(kit)
    local c = ids_of[kit]
    if c then return c end
    c = {}
    for i, r in ipairs(type(kit.rows) == "table" and kit.rows or {}) do c[i] = r.id end
    ids_of[kit] = c
    return c
end
Kit.armor_ids = armor_ids

-- The last good rules are kept while the slot has nothing (a new session
-- before the server's rules arrived) until Kit.forget_rules() (main.lua's menu
-- cleanup, no session). "rev" comes along so a host rule change re-dresses the
-- stand-ins (it is part of effective_remote's apply-key suffix).
local rules_last = nil
local RULES_NONE = { mode = 0, budget = 0, rev = 0 }
function Kit.rules()
    local I = ipc()
    local r = I and I.rec and I.rec("kit_rules")   -- `kit_rules` record (sidecar slot)
    if type(r) ~= "table" or (tonumber(r.rev) or 0) == 0 then return rules_last or RULES_NONE end
    rules_last = { mode = tonumber(r.mode) or 0, budget = tonumber(r.budget) or 0, rev = tonumber(r.rev) or 0 }
    return rules_last
end
function Kit.forget_rules() rules_last = nil end

local function match_round()
    local hs = HS()
    local v = hs and hs.view()   -- the typed session snapshot
    if not v then return 0, "lobby" end
    return v.round or 0, v.state or "lobby"
end

-- Exact assigned placement identity, scoped by the caller's native world.
function Kit.fighter_context(session, link, mode, st, world)
    if not (session and link and st and type(world) == "string" and world ~= ""
        and (link.my_peer_id or 0) > 0 and (st.seq or 0) > 0 and (st.match_id or 0) > 0
        and st.match_id == session.match_id and (st.life or 0) > 0
        and type(st.pawn) == "string" and st.pawn ~= "") then return nil end
    local phase = session.phase
    if not phase or phase < 1 or phase > 7 then return nil end
    local round = (phase == 1 or phase == 2) and session.round + 1 or session.round
    if st.round ~= round then return nil end
    local assigned
    for _, row in ipairs(session.rows or {}) do
        if row.peer_id == link.my_peer_id then assigned = row; break end
    end
    if not assigned or (st.spawn_id or 0) == 0 or st.spawn_id ~= assigned.spawn_id then return nil end
    if phase <= 2 then
        if st.life ~= 1 then return nil end
    else
        if not mode or mode.match_id ~= st.match_id or mode.round ~= st.round then return nil end
        local life
        for _, row in ipairs(mode.rows or {}) do
            if row.peer_id == link.my_peer_id then life = row.life; break end
        end
        if life ~= st.life then return nil end
    end
    return { key = table.concat({ world, tostring(st.match_id), tostring(st.round), tostring(st.life),
        tostring(st.spawn_id), st.pawn }, "|"), world = world, pawn = st.pawn, verified = st.verified == true }
end

-- Retain metadata, never an UObject, during a native fallback possession swap.
-- Resolve the verified fighter fresh and compare its native address/world.
function Kit.local_fighter(candidate, context, bound, swap, now, describe, resolve)
    local swapping = swap and (tonumber(swap.until_s) or 0) > now
    if not context then
        -- Keep only the old identity metadata while proof is unavailable; do
        -- not return any pawn. An exact context return may resume it later.
        if swapping then return nil, bound end
        return candidate, nil
    end
    if bound and bound.key ~= context.key then bound = nil end
    local function matches(pawn, address)
        local m = pawn and describe(pawn)
        return m and m.world == context.world and m.name == context.pawn
            and m.address ~= nil and (address == nil or m.address == address), m
    end
    local ok, meta = matches(candidate)
    if ok then
        if context.verified or bound then bound = { key = context.key, name = context.pawn, address = meta.address } end
        return candidate, bound
    end
    if swapping and bound and swap.keep == bound.name then
        local original = resolve(bound.name, bound.address)
        if matches(original, bound.address) then return original, bound end
        return nil, bound
    end
    -- Placement still names another actor: wait for assignment and possession
    -- to agree instead of dressing the stand-in with our kit.
    return nil, bound
end

-- --- catalogue -> classes ------------------------------------------------------

local slot_cache = {}   -- class path -> ArmorSlots_Enum value (or false)

-- The armour BP's own "Armor Slot" default (ABP_Armor_Master_C, spaced name).
local function armour_slot(path, cls)
    local s = slot_cache[path]
    if s ~= nil then return s or nil end
    local cdo
    pcall(function() cdo = cls:GetCDO() end)
    if not api.valid(cdo) then
        local full = api.expand_path(path)          -- /Game/.../BP_X.BP_X_C
        local pkg, name = full:match("^(.+)%.([^%.]+)$")
        if pkg then pcall(function() cdo = StaticFindObject(pkg .. ".Default__" .. name) end) end
    end
    local v
    if api.valid(cdo) then pcall(function() v = tonumber(cdo["Armor Slot"]) end) end
    slot_cache[path] = v or false
    if not v then Log("no 'Armor Slot' default for %s", path) end
    return v
end

local function item_path(id)
    local it = Cat and id and id ~= "" and Cat.items[id]
    return it and it.path or nil
end

-- Kit armour as main.lua "pieces": { {slot, path}, ... } (+ unresolved ids).
function Kit.pieces(kit)
    local out, missing = {}, {}
    for _, id in ipairs(armor_ids(kit)) do
        local path = item_path(id)
        local cls = path and api.resolve_class(path)
        local slot = cls and armour_slot(path, cls)
        if slot then out[#out + 1] = { slot, path } else missing[#missing + 1] = tostring(id) end
    end
    return out, missing
end

local function kit_paths(kit)
    local set = {}
    for _, id in ipairs(armor_ids(kit)) do
        local p = item_path(id); if p then set[p] = true end
    end
    return set
end

-- --- cosmetics -----------------------------------------------------------------

local function cos(kit, i)
    local c = type(kit.cos) == "table" and tonumber(kit.cos[i]) or 0
    return c or 0
end

-- Hair colour: the groom's default material exposes Melanin / Redness.
function Kit.apply_hair(pawn, kit)
    local h = cos(kit, 2)
    local e = Cat and Cat.hair[h + 1]
    if h == 0 or not e or not e[2] then return "default" end
    local mat = api.field(pawn, "Hair Mat")
    if not api.valid(mat) then return "no Hair Mat" end
    local ok = pcall(function()
        mat:SetScalarParameterValue(FName("Melanin"), e[2])
        mat:SetScalarParameterValue(FName("Redness"), e[3])
    end)
    return ok and e[1] or "FAIL"
end

-- Face: Willie_BP "Face Type" (int). Best effort — takes visual effect only if
-- the game re-reads it on setup; logged for verification.
function Kit.apply_face(pawn, kit)
    local f = cos(kit, 1)
    if f == 0 then return "default" end
    local ok = pcall(function() pawn["Face Type"] = f end)
    return ok and ("Face Type=" .. f) or "FAIL"
end

-- Cloth tint: { colour, dark colour } that main.lua's passport builder puts
-- into the fabric/leather colours of every kit piece's passport. nil (index
-- 0) keeps the colours from the game's own loadout passports.
function Kit.tint(kit)
    local t = cos(kit, 3)
    local e = Cat and Cat.tints[t + 1]
    if t == 0 or not e or not e[2] then return nil end
    return { { e[2], e[3], e[4], 1 }, { e[2] * 0.5, e[3] * 0.5, e[4] * 0.5, 1 } }
end

-- Is `path` (short class path) a selectable catalogue item?
local cat_paths
function Kit.is_catalog_path(path)
    if not Cat or not path then return false end
    if not cat_paths then
        cat_paths = {}
        for _, it in pairs(Cat.items) do if it.path then cat_paths[it.path] = true end end
    end
    return cat_paths[path] == true
end

-- --- weapons ---------------------------------------------------------------------

local function current_weapon(pawn, side)
    for _, f in ipairs(side == "R" and { "Weapon R", "Weapon R_0" } or { "Weapon L", "Weapon L_0" }) do
        local x = api.field(pawn, f)
        if api.valid(x) then return x end
    end
    return nil
end

-- Give `pawn` the weapon class `path` in hand `side` ("R"/"L"); nil = empty hand.
-- The native equip function builds from the complete canonical passport.
-- The actor fallback receives that same passport before native construction.
function Kit.give_weapon(pawn, side, path)
    local fname = side == "R" and "Set Up Right Hand Weapon" or "Set Up Left Hand Weapon"
    local cur = current_weapon(pawn, side)
    if not path then
        if api.set_hand_passport and not api.set_hand_passport(pawn, side, nil) then return "FAIL hand passport" end
        if cur then
            if api.destroy_hand_weapon then api.destroy_hand_weapon(cur)   -- bumps standin_weapons
            else pcall(function() cur:K2_DestroyActor() end) end   -- unsafe: ok a hand weapon actor, never a Willie
            return "stripped"
        end
        return "none"
    end
    local cls = api.resolve_class(path)
    if not cls then return "FAIL noclass" end
    -- The game re-arms the hands from the character passport after a setup;
    -- keep it in line with the kit (see main.lua set_hand_passport).
    local pass = api.weapon_passport_for and api.weapon_passport_for(cls)
    if not pass then return "FAIL native weapon defaults unavailable" end
    if api.set_hand_passport and not api.set_hand_passport(pawn, side, cls, pass) then return "FAIL hand passport" end
    if cur then
        local cp = ""; pcall(function() cp = api.class_path(cur:GetClass()) end)
        if cp == path and api.weapon_passport_matches and api.weapon_passport_matches(cur, pass) then return "same" end
    end
    -- The class form: the game spawns the weapon from the class/passport
    -- itself and attaches it to the hand socket.
    local ok, err = pcall(api.bp_call, pawn, fname, cls, nil, false, true, pass)
    if ok and cur and api.hand_weapon_replaced then api.hand_weapon_replaced() end
    if ok then
        local now = current_weapon(pawn, side)
        local np = ""; pcall(function() np = api.class_path(now:GetClass()) end)
        if np == path and api.weapon_passport_matches and api.weapon_passport_matches(now, pass) then return "ok" end
        err = "native hand passport mismatch"
    end
    -- Fallback: spawn the actor ourselves and hand it over.
    local a
    local ok2, err2 = pcall(function()
        a = api.spawn_weapon_actor(pawn, cls, pass)
        if not a then error("spawn failed") end
        api.bp_call(pawn, fname, cls, a, false, true, pass)
        if cur and api.hand_weapon_replaced then api.hand_weapon_replaced() end
        local now = current_weapon(pawn, side)
        local np = now and api.class_path(now:GetClass()) or ""
        if np ~= path or not api.weapon_passport_matches or not api.weapon_passport_matches(now, pass) then
            error("native hand passport mismatch")
        end
    end)
    if ok2 then return "ok(actor)" end
    if api.valid(a) then pcall(function() a:K2_DestroyActor() end) end
    return "FAIL " .. tostring(err) .. " / " .. tostring(err2)
end

-- --- stand-ins ---------------------------------------------------------------------

local function copy_filtered_pieces(list, allow)
    local out = {}
    for _, p in ipairs(list or {}) do
        if type(p) == "table" and allow[p[2]] then out[#out + 1] = p end
    end
    return out
end

-- (loadout to show on peer `id`'s stand-in, key suffix) or nil.
-- `L` is the owner's replicated appearance (peer_loadout record) or nil.
function Kit.effective_remote(id, L)
    if not Cat then return L, "" end
    local kit = Kit.read(tonumber(id))
    if not kit or kit.class == "none" then return L, "" end
    local rules = Kit.rules()
    -- The rules' mode and rev are part of the key: a host rule change
    -- (FREE <-> CLASSES, a new budget) re-dresses the stand-ins.
    local suffix = "k" .. tostring(kit.rev) .. "m" .. tostring(rules.mode) .. "r" .. tostring(rules.rev)
    if L then
        if rules.mode == Cat.MODE_FREE then
            local E = {}; for k, v in pairs(L) do E[k] = v end
            E.cos_kit = kit
            E.tint = Kit.tint(kit)
            return E, suffix
        end
        -- Enforced rules: show only armour that belongs to the validated kit.
        local allow = kit_paths(kit)
        local E = {}
        for k, v in pairs(L) do E[k] = v end
        local before = #(L.p or {})
        E.p = copy_filtered_pieces(L.p, allow)
        local pass = {}
        for _, a in ipairs(L.a or {}) do
            if type(a) == "table" and type(a[2]) == "table" and allow[a[2].class] then pass[#pass + 1] = a end
        end
        E.a = pass
        E.cos_kit = kit
        E.tint = Kit.tint(kit)
        local note = tostring(L.v) .. "/" .. tostring(kit.rev) .. "/" .. (before - #E.p)
        if before > #E.p and Kit._hid_note[id] ~= note then
            Kit._hid_note[id] = note
            Log("peer %s: hid %d armour piece(s) not in their validated %s kit", tostring(id), before - #E.p, kit.class)
        end
        return E, suffix
    end
    -- No appearance yet: show the kit itself.
    local pieces = Kit.pieces(kit)
    return { v = 0, p = pieces, a = {}, tint = Kit.tint(kit), w = {},
             kit_w = { R = item_path(kit.r), L = item_path(kit.l) }, cos_kit = kit }, suffix
end

-- --- stand-in apply keys (pure; main.lua apply_remote) ---------------------------------
--
-- Armour and weapons are keyed separately. A held-item change (a
-- pickup, a drop, a world item in hand) changes only the weapon key, and
-- main.lua then runs only the hand work (yield_hand / apply_weapon), never
-- "Set Up Armor" (Clear Previous), which respawns every armour actor on the
-- stand-in mid-fight (a hitch, flicker, changing armour collision).

-- Deterministic text of a value: arrays in order, record tables by sorted field name.
local function ser(v)
    if type(v) ~= "table" then return tostring(v) end
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    local out = {}
    for i, k in ipairs(keys) do out[i] = (math.type(k) == "integer" and "" or (tostring(k) .. "=")) .. ser(v[k]) end
    return "{" .. table.concat(out, ",") .. "}"
end

-- Everything "Set Up Armor" is built from: pieces, passports, tint, hair kit.
-- (The kit / rules part is in effective_remote's suffix.)
function Kit.armour_sig(L)
    if type(L) ~= "table" then return "-" end
    local parts = {}
    for _, p in ipairs(L.p or {}) do
        if type(p) == "table" then parts[#parts + 1] = tostring(p[1]) .. "=" .. tostring(p[2]) end
    end
    parts[#parts + 1] = "|"
    for _, a in ipairs(L.a or {}) do
        if type(a) == "table" then parts[#parts + 1] = tostring(a[1]) .. ":" .. ser(a[2]) end
    end
    parts[#parts + 1] = "|t" .. ser(L.tint)
    return table.concat(parts, ";")
end

-- What the hands are built from: the owner's held classes (or the kit's
-- weapons), the world items HSMPWorld says they hold (hsuf), and hands
-- stripped for a dropped weapon (strip = { R = true, L = true }).
function Kit.weapon_sig(L, hsuf, strip)
    if type(L) ~= "table" then return "-" end
    local w, kw = L.w or {}, L.kit_w
    local function c(e)
        if Kit.weapon_passport_key then return Kit.weapon_passport_key(e) end
        return type(e) == "table" and tostring(e.class) or "-"
    end
    return string.format("R=%s,L=%s,kR=%s,kL=%s%s%s%s", c(w.R), c(w.L), kw and tostring(kw.R) or "-",
        kw and tostring(kw.L) or "-", hsuf or "", (strip and strip.R) and "|sR" or "", (strip and strip.L) and "|sL" or "")
end

-- Dynamic world items in the text form of HSMPWorld's world manifest
-- ([id,chash,x,y,z,dyn,"class"], dyn = the peer whose weapon left its hand).
-- Returns { {id, peer, leaf = "X_C"} } for dyn ~= 0.
function Kit.parse_dyn(raw)
    local out = {}
    if type(raw) ~= "string" then return out end
    local NUM = "(-?[%d%.]+[eE]?[-+]?%d*)"
    local pat = "%[(%d+),(%d+)," .. NUM .. "," .. NUM .. "," .. NUM .. ',(%d+),"([^"]*)"%]'
    for id, _, _, _, _, dyn, cls in raw:gmatch(pat) do
        local d = tonumber(dyn) or 0
        if d ~= 0 then out[#out + 1] = { id = tonumber(id), peer = d, leaf = cls:match("([^%./]+)$") or cls } end
    end
    return out
end

-- May the menu delete the kit / loadout state an earlier session left
-- behind? Only when that session is provably over: a terminal sidecar status,
-- no link record for >= 2 s, or a sidecar whose header heartbeat stopped for
-- >= 15 s. A beating sidecar is never "over" (a Welcome arriving right then
-- would lose its fresh kit / rules state).
-- S: a shared/hsmp_session.lua tracker (polled by the caller).
Kit.CLEAN_ABSENT_S, Kit.CLEAN_STALE_S = 2.0, 15.0
function Kit.menu_cleanup_ok(S, now)
    if not S then return false end
    if S:terminal() then return true end
    -- A beating sidecar (header heartbeat) is a session that may still be writing.
    if S:fresh() then S.cleanup_dead_since = nil; return false end
    -- No link record (no sidecar) for CLEAN_ABSENT_S, or a link whose sidecar stopped
    -- beating for CLEAN_STALE_S: that session is over.
    S.cleanup_dead_since = S.cleanup_dead_since or now
    return now - S.cleanup_dead_since >= (S.exists and Kit.CLEAN_STALE_S or Kit.CLEAN_ABSENT_S)
end

-- --- own pawn ----------------------------------------------------------------------

local own = { key = nil, pending = nil, deferred = false }

local verify_armour   -- forward

local function pawn_addr(p)
    local a = "?"; pcall(function() a = tostring(p:GetAddress()) end)
    return a
end

-- Bare hands are an actor too: after a knockdown Half Sword drops the held
-- weapon and "Weapon R/L" becomes Weapon_Fists_C. Fists/feet are never a
-- held kit weapon.
local function is_bare(cls)
    return cls == nil or cls == "" or cls:find("Weapon_Fists", 1, true) ~= nil or cls:find("Weapon_Feet", 1, true) ~= nil
end
Kit.is_bare = is_bare

-- The slot maps are what we wrote, so they prove nothing; check the game's
-- own results: "Currently Equipped Armor", which "Set Up Armor" fills with
-- the pieces it actually spawned (source "equipped")
-- when it reports anything, else the count of worn armour meshes.
-- A hand counts only when it holds the kit class and complete native passport.
local function weapon_ok(pawn, side, path)
    local cur = current_weapon(pawn, side)
    local cp = ""
    if cur then pcall(function() cp = api.class_path(cur:GetClass()) end) end
    if not path then return is_bare(cp), cp end
    if cp ~= path or is_bare(cp) then return false, cp end
    local cls = api.resolve_class(path)
    local pass = cls and api.weapon_passport_for and api.weapon_passport_for(cls)
    return pass ~= nil and api.weapon_passport_matches ~= nil and api.weapon_passport_matches(cur, pass), cp
end

-- Both hands against the kit: ok, { "R (Weapon_Fists_C)", ... }, { "R", ... }.
local function hands_check(pawn, kit)
    local bad, sides = {}, {}
    for _, side in ipairs({ "R", "L" }) do
        local path = item_path(side == "R" and kit.r or kit.l)
        local ok, cp = weapon_ok(pawn, side, path)
        if not ok then
            bad[#bad + 1] = side .. " (" .. (cp ~= "" and cp:match("([^/%.]+)$") or "empty") .. ")"
            sides[#sides + 1] = side
        end
    end
    return #bad == 0, bad, sides
end
Kit.hands_check = hands_check

-- A kit weapon in hand must also be seen: true / false / nil (no weapon or
-- no visibility probe).
local function hand_visible(pawn, side)
    local cur = current_weapon(pawn, side)
    if not cur then return nil end
    -- "Still no weapons": the deep probe (main.lua weapon_shown: hidden,
    -- root, visible mesh, origin, distance from the hand bone) when present.
    if api.weapon_shown then
        local ok, why = api.weapon_shown(cur, pawn, side)
        return ok, why, cur
    end
    if not api.actor_visible then return nil end
    return api.actor_visible(cur), nil, cur
end
Kit.hand_visible = hand_visible

local function verify(pawn, kit, retain_weapon)
    local wr, cr = weapon_ok(pawn, "R", item_path(kit.r))
    local wl, cl = weapon_ok(pawn, "L", item_path(kit.l))
    local ok, missing, worn = verify_armour(pawn, kit)
    if not wr then missing[#missing + 1] = "R hand (" .. (cr ~= "" and cr:match("([^/]+)$") or "empty") .. ")" end
    if not wl then missing[#missing + 1] = "L hand (" .. (cl ~= "" and cl:match("([^/]+)$") or "empty") .. ")" end
    -- In hand but invisible (hidden while the pawn was being dressed, or a
    -- pooled actor hidden elsewhere): unhide, and count it as missing until
    -- the next verify sees it.
    local hid, rearm = {}, {}
    for _, s in ipairs({ "R", "L" }) do
        local held_ok = (s == "R") and wr or wl
        local path = item_path(s == "R" and kit.r or kit.l)
        if held_ok and path then
            local vis, why, cur = hand_visible(pawn, s)
            if vis == false then
                if why == nil or why == "hidden" or why == "root invisible" then
                    hid[#hid + 1] = s
                else
                    -- in hand, not hidden, but not seen where the hand is (a
                    -- broken grip after a teleport, a half-built spawn at the
                    -- origin, no mesh): take it away so the next apply gives
                    -- a fresh one (give_weapon would answer "same" otherwise).
                    rearm[#rearm + 1] = s .. ":" .. tostring(why)
                    if not retain_weapon then
                        if api.set_hand_passport then api.set_hand_passport(pawn, s, nil) end
                        if api.destroy_hand_weapon then api.destroy_hand_weapon(cur) end
                    end
                end
            end
        end
    end
    if #hid > 0 or #rearm > 0 then
        local n = (#hid > 0 and api.unhide_carried) and api.unhide_carried(pawn) or 0
        for _, s in ipairs(hid) do missing[#missing + 1] = s .. " hand HIDDEN" end
        for _, s in ipairs(rearm) do missing[#missing + 1] = s .. " hand NOT SHOWN" end
        if #hid > 0 then Log("own kit: weapon in %s hand was hidden; unhid %d carried actor(s)", table.concat(hid, "+"), n) end
        if #rearm > 0 then Log("own kit: weapon not shown at the hand (%s); %s", table.concat(rearm, ", "),
            retain_weapon and "retained for same-actor re-arm" or "removed, re-arming") end
        return false, missing, worn
    end
    return ok and wr and wl, missing, worn
end

verify_armour = function(pawn, kit)
    if api.armour_passports_match then
        local pieces, unresolved = Kit.pieces(kit)
        local ok, missing = api.armour_passports_match(pawn, pieces, Kit.tint(kit))
        for _, id in ipairs(unresolved) do missing[#missing + 1] = id .. " unresolved" end
        return ok and #unresolved == 0, missing, api.worn_count(pawn)
    end
    local want = kit_paths(kit)
    local nwant = 0; for _ in pairs(want) do nwant = nwant + 1 end
    local worn = api.worn_count(pawn)
    local game = api.read_source(pawn, "equipped")
    if #game > 0 then
        local have = {}
        for _, p in ipairs(game) do have[p[2]] = true end
        local missing = {}
        for path in pairs(want) do
            if not have[path] then missing[#missing + 1] = path:match("([^/]+)$") end
        end
        return #missing == 0, missing, worn
    end
    if worn >= nwant then return true, {}, worn end
    return false, { string.format("worn meshes %d < %d pieces", worn, nwant) }, worn
end

local function apply_own(pawn, kit)
    local pieces, unresolved = Kit.pieces(kit)
    local L = { p = pieces, a = {}, tint = Kit.tint(kit) }
    local armour_ok, n, ares = api.apply_armour(pawn, L)
    local wr = Kit.give_weapon(pawn, "R", item_path(kit.r))
    local wl = Kit.give_weapon(pawn, "L", item_path(kit.l))
    local hair = Kit.apply_hair(pawn, kit)
    local face = Kit.apply_face(pawn, kit)
    Log("own kit %s rev=%s applied: pieces=%d%s %s | R=%s L=%s | hair=%s face=%s",
        tostring(kit.class), tostring(kit.rev), n,
        #unresolved > 0 and (" UNRESOLVED=" .. table.concat(unresolved, ",")) or "",
        ares, wr, wl, hair, face)
    return armour_ok and not wr:find("^FAIL") and not wl:find("^FAIL")
end

-- Dressing timing: players must never visibly spawn bare and get dressed
-- seconds later. main.lua hides every new Willie of an MP arena at BeginPlay and
-- calls this every 100 ms: the kit goes on as soon as the pawn is ours, is
-- verified 0.3 s later (armour classes in "Currently Equipped Armor" + both
-- hands), and only then is the pawn revealed. For 3 s after that the kit is
-- re-checked, so a native re-arm right after spawn is undone immediately.
local VERIFY_AFTER, MAX_TRIES, STABLE_S, BUSY_WAIT = 0.3, 6, 3.0, 0.5
Kit.VERIFY_AFTER, Kit.MAX_TRIES, Kit.STABLE_S = VERIFY_AFTER, MAX_TRIES, STABLE_S

-- Dev only (HSMP_DEV=1): HSMP_DEV_KIT_DELAY_S=<s> holds our kit back until the
-- pawn has been hidden that long, so the safety reveal always comes first:
-- the late-kit path of a slow link, on every arena load (IO-1 soak).
local function env_num(k)
    local v = os.getenv(k)
    return v and tonumber((v:gsub("%s", ""))) or nil
end
Kit.DEV_KIT_DELAY_S = ((os.getenv("HSMP_DEV") or ""):match("^%s*1%s*$") and env_num("HSMP_DEV_KIT_DELAY_S")) or 0

local function nm(x) local n = "-"; if x then pcall(function() n = x:GetFName():ToString() end) end; return n end
local function clock() return (api and api.now or os.clock)() end

-- --- the Director's kit evidence: bus key "kit_status" (docs/development/subsystems/director.md)
-- A typed record (schema loadout.rs KitStatus): pawn, rev, ok, armour_n, exp_armour_n,
-- r_class, l_class, r_visible, l_visible (hand_vis: 0 unknown, 1 shown, 2 hidden), tries,
-- error, kit, stable, round, t. Written after every verify (and when dressing starts /
-- gives up / is undone by a native re-arm), so the Director's kit step reads the real
-- state of the current pawn. ok=true only when every kit armour class and both hands
-- check out.

local function short_class(x)
    local c = nil
    if x then pcall(function() c = x:GetClass():GetFName():ToString() end) end
    return c
end

local function vis_code(v) if v == true then return 1 elseif v == false then return 2 end return 0 end

-- Pure: the kit_status record for the fields `f` (tests call it directly).
function Kit.status_record(f)
    return {
        pawn = tostring(f.pawn or ""), rev = math.tointeger(tonumber(f.rev) or 0) or 0, ok = f.ok and true or false,
        armour_n = math.floor(tonumber(f.armour_n) or 0), exp_armour_n = math.floor(tonumber(f.exp_armour_n) or 0),
        r_class = f.r_class or "", l_class = f.l_class or "",
        r_visible = vis_code(f.r_visible), l_visible = vis_code(f.l_visible),
        tries = math.floor(tonumber(f.tries) or 0), error = f.error and tostring(f.error) or "",
        kit = tostring(f.kit or "none"), stable = f.stable and true or false,
        round = math.floor(tonumber(f.round) or 0), t = tonumber(f.t) or 0,
    }
end

local status_sig = nil   -- last written content without "t" (no rewrite every 100 ms)
-- final: a verdict (verified, or given up) -> also the kit_verified event.
-- ok is never true while a hand misses its kit weapon (fists do not count),
-- whatever the caller thought: r_class/l_class are read from the real actors.
local function write_status(pawn, kit, ok, tries, err, stable, final)
    local I = ipc()
    if not (I and I.bus_put) then return end
    if ok and kit then
        local hands, bad = hands_check(pawn, kit)
        if not hands then
            ok = false
            err = err or ("kit weapon not in hand: " .. table.concat(bad, ","))
        end
    end
    local want = kit and kit_paths(kit) or {}
    local nwant = 0; for _ in pairs(want) do nwant = nwant + 1 end
    local game = api.read_source and api.read_source(pawn, "equipped") or {}
    local armour_n = #game > 0 and #game or (api.worn_count and api.worn_count(pawn) or 0)
    local f = {
        pawn = nm(pawn), rev = kit and kit.rev, ok = ok, armour_n = armour_n, exp_armour_n = nwant,
        r_class = short_class(current_weapon(pawn, "R")), l_class = short_class(current_weapon(pawn, "L")),
        r_visible = hand_visible(pawn, "R"), l_visible = hand_visible(pawn, "L"),
        tries = tries, error = err, kit = kit and kit.class or "none", stable = stable, round = (match_round()),
        t = clock(),
    }
    local rec = Kit.status_record(f)
    local t = rec.t
    rec.t = nil
    local sig = ser(rec)
    rec.t = t
    if sig == status_sig then return end
    status_sig = sig
    I.bus_put("kit_status", rec)
    if api.ev and final then
        pcall(api.ev, "kit_verified", { who = "self", armour_n = f.armour_n, r_class = f.r_class or "None",
            l_class = f.l_class or "None", ok = ok and true or false, exp_armour_n = nwant, round = f.round,
            kit = f.kit, tries = tries, error = err, r_visible = f.r_visible, l_visible = f.l_visible })
    end
end
Kit._write_status = write_status

-- --- after the stability window: watch the hands for the whole world ---------------
--
-- Half Sword drops held weapons on a knockdown or a hard contact. Before the
-- round is Live (countdown / paused) or while HSMPSync's spawn protection holds
-- (spawn_status record: protect_until unset or in the future), a dropped kit
-- weapon is put back in hand at once: the dropped actor itself, never
-- destroyed (see rearm below). During Live a drop is
-- gameplay: no auto re-arm, but kit_status says ok=false with the real
-- hand classes, the log says so once, and a pawn_state event is written.
local WATCH_EVERY, MAX_REARMS = 0.5, 8
Kit.WATCH_EVERY, Kit.MAX_REARMS = WATCH_EVERY, MAX_REARMS


-- true, why while a dropped kit weapon must be put back.
local function rearm_window(pawn, now)
    local _, st = match_round()
    if st == "countdown" or st == "paused" then return true, st end
    local ipc = rawget(_G, "HSMP_IPC")
    local bt = (api and api.bus_table) or (ipc and ipc.bus_table)
    local ss = bt and bt("spawn_status")   -- typed bus record (HSMPSync)
    if type(ss) == "table" and (ss.seq or 0) ~= 0 and ss.pawn == nm(pawn) then
        if not ss.has_protect_until then return true, "spawn protection" end
        if now < ss.protect_until then return true, "spawn protection" end
    end
    return false, st
end
Kit._rearm_window = rearm_window

local function state_event(pawn, at, reason)
    if not api.ev then return end
    local function num(k) local v = tonumber(api.field(pawn, k)); return v and math.floor(v * 10 + 0.5) / 10 or nil end
    pcall(api.ev, "pawn_state", { at = at, who = "self", pawn = nm(pawn), round = (match_round()),
        consciousness = num("Consciousness"), downed = api.field(pawn, "Downed") == true,
        fallen = api.field(pawn, "Fallen") == true, health = num("Health"),
        weapon_r = short_class(current_weapon(pawn, "R")) or "None",
        weapon_l = short_class(current_weapon(pawn, "L")) or "None", reason = reason })
end

local function same_actor(a, b)
    if a == nil or b == nil then return false end
    if rawequal(a, b) then return true end
    local ok, eq = pcall(function() return a == b end)
    if ok and eq then return true end
    local aa, ba = pawn_addr(a), pawn_addr(b)
    return aa ~= "?" and aa == ba
end

-- Put the dropped actor itself back in hand (the same "Set Up ...
-- Hand Weapon" call the actor form of give_weapon uses). Returns a result
-- string, or nil when it did not end up in hand. The optional second result
-- distinguishes a refusal before native setup from attempted, unproved native
-- completion: native setup can subtract weight before a later failure.
function Kit.reequip(pawn, side, actor, path)
    local fname = side == "R" and "Set Up Right Hand Weapon" or "Set Up Left Hand Weapon"
    local prepared, input = pcall(function()
        if not api.valid(actor) then return nil end
        local cls = api.resolve_class(path)
        if not cls then return nil end
        local pass = api.weapon_passport_for and api.weapon_passport_for(cls, actor)
        if not pass then return nil end
        local same = same_actor(current_weapon(pawn, side), actor)
        -- The native OR condition destroys grip0 even with Destroy Previous
        -- false. Refuse it before changing the hand passport.
        if same then
            local grip = api.field(pawn, side .. "_GripType_Current")
            if type(grip) ~= "number" or not math.tointeger(grip) or grip <= 0 or grip > 255 then return nil end
        end
        if api.set_hand_passport and not api.set_hand_passport(pawn, side, cls, pass) then return nil end
        return {cls=cls,pass=pass,same=same}
    end)
    if not prepared or not input then return nil, "preflight" end
    local ok = pcall(api.bp_call, pawn, fname, input.cls, actor, false, not input.same, input.pass)
    if not ok then return nil, "attempted" end
    local read, result = pcall(function()
        if api.valid(actor) and same_actor(current_weapon(pawn, side), actor) then
            return "ok(same actor) retained=" .. nm(actor) .. "@" .. pawn_addr(actor)
        end
    end)
    if read and result then
        return result, "complete"
    end
    return nil, "attempted"
end

-- Contract (docs/development/subsystems/spawns.md): a kit weapon that left the
-- own hand is never destroyed here. HSMPWorld may already have registered it as
-- a dynamic world item (100 ms after it went loose) and peers spawned copies;
-- nothing retires a dynamic id, so destroying it leaves a pickable ghost on
-- every other screen. The dropped actor is re-equipped instead (HSMPWorld
-- then sees its own item picked up again: a "picked up" claim, one weapon on
-- every screen). Retain a valid recoverable actor when native re-equipping
-- needs another try. Replace only an invalid/wrong-class actor or one held by
-- somebody else; a dropped world item is never destroyed here.
local function rearm(pawn, kit, sides)
    local res = {}
    for _, side in ipairs(sides) do
        local path = item_path(side == "R" and kit.r or kit.l)
        local old = own.held and own.held[side]
        local r
        if path and old and api.valid(old) then
            local held, cp = nil, ""
            pcall(function()
                local value = api.field(old, "Is Held")
                if type(value) == "boolean" then held = value end
            end)
            pcall(function() cp = api.class_path(old:GetClass()) end)
            if cp == path and (held == false or same_actor(current_weapon(pawn, side), old)) then
                r = Kit.reequip(pawn, side, old, path)
                if not r then r = "waiting(same actor retained) retained=" .. nm(old) .. "@" .. pawn_addr(old) end
            elseif cp == path and held == nil then
                r = "waiting(actor ownership unavailable) retained=" .. nm(old) .. "@" .. pawn_addr(old)
            elseif cp == "" then
                r = "waiting(actor class unavailable) retained=" .. nm(old) .. "@" .. pawn_addr(old)
            end
        end
        r = r or tostring(Kit.give_weapon(pawn, side, path))
        local now_w = current_weapon(pawn, side)
        if own.held and now_w and not is_bare(short_class(now_w)) then own.held[side] = now_w end
        res[#res + 1] = side .. "=" .. r
    end
    return table.concat(res, " ")
end

local function watch_hands(pawn, now)
    local wt = own.watch
    if now < wt.next_t then return end
    wt.next_t = now + WATCH_EVERY
    local ok, bad, sides = hands_check(pawn, wt.kit)
    for _, side in ipairs({ "R", "L" }) do
        if item_path(side == "R" and wt.kit.r or wt.kit.l) and hand_visible(pawn, side) == false then
            local found = false; for _, s in ipairs(sides) do if s == side then found = true end end
            if not found then bad[#bad + 1] = side .. " hand NOT SHOWN"; sides[#sides + 1] = side end
            ok = false
        end
    end
    if ok then
        if wt.down then
            Log("own kit %s: kit weapons in hand again (was %s)", tostring(wt.kit.class), wt.down)
            wt.down, wt.gave_up = nil, nil
            write_status(pawn, wt.kit, true, wt.tries, nil, true, false)
        end
        return
    end
    local why = table.concat(bad, ",")
    local window, wwhy = rearm_window(pawn, now)
    if window then
        if wt.rearms < MAX_REARMS then
            wt.rearms = wt.rearms + 1
            write_status(pawn, wt.kit, false, wt.tries, "re-arming: dropped " .. why, false, false)
            state_event(pawn, "rearm", why)
            local res = rearm(pawn, wt.kit, sides)
            Log("own kit %s: kit weapon dropped before the round went Live (%s; window: %s); re-equipped %s (%d/%d)",
                tostring(wt.kit.class), why, tostring(wwhy), res, wt.rearms, MAX_REARMS)
            wt.down = why
            wt.next_t = now + Kit.VERIFY_AFTER
        elseif not wt.gave_up then
            wt.gave_up = true
            Log("own kit %s: kit weapon still not in hand after %d re-arms (%s); giving up until the next spawn",
                tostring(wt.kit.class), MAX_REARMS, why)
            write_status(pawn, wt.kit, false, wt.tries, "re-arm gave up: " .. why, false, true)
        end
        return
    end
    if wt.down ~= why then
        wt.down = why
        Log("own kit %s: kit weapon dropped during the live round (%s); no auto re-arm (a drop is gameplay)",
            tostring(wt.kit.class), why)
        write_status(pawn, wt.kit, false, wt.tries, "dropped during Live: " .. why, true, false)
        state_event(pawn, "weapon_drop", why)
    end
end

-- Called every 100 ms from main.lua (game thread).
function Kit.tick_local()
    if not Cat then return end
    if not api.in_arena() then Kit.on_world_change(); return end
    -- Only while an MP session is live. A leftover link record /
    -- peer_kit record from an earlier session must never rewrite a
    -- single-player pawn's passport (the native "Save Game" would keep it).
    if api.mp_live and not api.mp_live() then
        Kit.on_world_change()
        return
    end
    local pawn = api.local_pawn()
    if not pawn then return end
    local pid = Kit.my_peer_id()
    local kit = Kit.read(pid)
    if kit and Kit.DEV_KIT_DELAY_S > 0 and api.dress_age then
        local age = api.dress_age(pawn)
        if age and age < Kit.DEV_KIT_DELAY_S then kit = nil end
    end
    if not kit then
        -- Connected but our kit has not arrived yet: not done. The
        -- Director's kit step waits (and reports kit_error after kit_s) instead
        -- of passing and dressing during Live.
        write_status(pawn, nil, false, 0, pid == 0 and "no peer id yet" or "no kit yet", false, false)
        own.bad_status = true
        return
    end
    if kit.class == "none" then
        if api.reveal then api.reveal(pawn, "no kit") end
        write_status(pawn, nil, true, 0, nil, true, false)   -- nothing to dress: the kit step must not wait
        return
    end
    local _, mstate = match_round()
    local addr = pawn_addr(pawn)
    -- One dress per pawn (and kit revision), not per match round: the session
    -- round goes 0 -> 1 when the countdown turns Live, and keying on it would
    -- re-dress every pawn at the very moment the round starts (hands empty in
    -- the first seconds of the fight). A new round is a new world and a new pawn
    -- (the Director reloads the arena; on_world_change() clears the key).
    local key = addr .. "|" .. tostring(kit.rev)
    local context = api.fighter_context and api.fighter_context(pawn)
    -- Acquiring placement proof does not re-dress this same pawn. An actual
    -- life/assignment/world change does, even when a native address is reused.
    if context and own.context and own.context ~= context then own.key = nil end
    if context then own.context = context end
    local now = clock()
    if key ~= own.key then
        local prev_addr = (own.key or ""):match("^([^|]*)|")
        local respawn = prev_addr ~= addr
        if not respawn and mstate == "live" then
            -- Kit / rules changed mid-round: takes effect at the next reset.
            if not own.deferred then
                own.deferred = true
                Log("kit rev %s arrived during a live round; applying at next round reset", tostring(kit.rev))
            end
            return
        end
        own.key, own.deferred, own.stable, own.watch, own.held = key, false, nil, nil, nil
        own.pending = { kit = kit, due = now, tries = 0, verify_at = nil }
        if kit.verdict and tonumber(kit.verdict) ~= 0 then
            Log("NOTE: server replaced your selection (%s) -> %s", tostring(kit.reason), tostring(kit.class))
        end
        write_status(pawn, kit, false, 0, "dressing", false, false)
    elseif own.bad_status then
        -- An error status was written for this same pawn/kit in between (no
        -- peer id / no kit for a tick): put the real state back.
        if own.watch and not own.pending and not own.stable then
            write_status(pawn, own.watch.kit, true, own.watch.tries, nil, true, false)
        else
            write_status(pawn, kit, false, 0, "dressing", false, false)
        end
    end
    own.bad_status = nil
    -- Stability window after a verified dress: undo a native re-arm at once.
    local st = own.stable
    if st and not own.pending then
        if now < st.next_t then return end
        st.next_t = now + VERIFY_AFTER
        local armour_ok = verify_armour(pawn, st.kit)
        local ok, missing = verify(pawn, st.kit, armour_ok)
        if not ok then
            if armour_ok then
                local _, _, sides = hands_check(pawn, st.kit)
                if #sides == 0 then
                    for _, side in ipairs({ "R", "L" }) do
                        if item_path(side == "R" and st.kit.r or st.kit.l) and hand_visible(pawn, side) == false then
                            sides[#sides + 1] = side
                        end
                    end
                end
                local window = rearm_window(pawn, now)
                st.rearms = st.rearms or 0
                if window and #sides > 0 and st.rearms < MAX_REARMS then
                    st.rearms = st.rearms + 1
                    local res = rearm(pawn, st.kit, sides)
                    Log("own kit %s: initial hand recovery %s (%d/%d); outfit retained",
                        tostring(st.kit.class), res, st.rearms, MAX_REARMS)
                end
                write_status(pawn, st.kit, false, st.tries,
                    st.rearms >= MAX_REARMS and "initial hand recovery gave up" or
                    ("initial hand recovery: " .. table.concat(missing, ",")), false, false)
                return
            end
            Log("own kit %s: native re-arm undid %s within %.1f s of dressing; re-dressing",
                tostring(st.kit.class), table.concat(missing, ","), now - st.dressed_t)
            own.pending = { kit = st.kit, due = now, tries = 0, verify_at = nil }
            own.stable = nil
            write_status(pawn, st.kit, false, 0, "re-dressing: native re-arm undid " .. table.concat(missing, ","),
                false, false)
        elseif now >= st.until_t then
            own.stable = nil
            write_status(pawn, st.kit, true, st.tries, nil, true, false)
            own.watch = { kit = st.kit, tries = st.tries, next_t = now, rearms = st.rearms or 0 }
        else
            write_status(pawn, st.kit, true, st.tries, nil, false, false)
        end
        return
    end
    if own.watch and not own.pending then
        watch_hands(pawn, now)
        return
    end
    local p = own.pending
    if not p then return end
    if p.verify_at then
        if now < p.verify_at then return end
        local ok, missing, worn = verify(pawn, p.kit)
        if ok then
            local ms = api.reveal and api.reveal(pawn, "kit verified")
            Log("own kit %s verified (armour classes all equipped, worn meshes=%d, R=%s L=%s)%s",
                tostring(p.kit.class), worn, nm(current_weapon(pawn, "R")), nm(current_weapon(pawn, "L")),
                ms and string.format(" - dressed in %d ms", ms) or "")
            own.pending = nil
            own.held = { R = current_weapon(pawn, "R"), L = current_weapon(pawn, "L") }
            own.stable = { kit = p.kit, until_t = now + STABLE_S, next_t = now + VERIFY_AFTER, dressed_t = now,
                           tries = p.tries }
            write_status(pawn, p.kit, true, p.tries, nil, false, true)
            return
        end
        if p.tries >= MAX_TRIES then
            Log("own kit %s: still missing after %d tries: %s — press Ctrl+F7 for a [gear] dump",
                tostring(p.kit.class), p.tries, table.concat(missing, ","))
            if api.reveal then api.reveal(pawn, "kit incomplete") end
            own.pending = nil
            write_status(pawn, p.kit, false, p.tries, string.format("kit incomplete after %d tries: %s", p.tries,
                table.concat(missing, ",")), false, true)
            return
        end
        Log("own kit %s: missing %s; re-applying", tostring(p.kit.class), table.concat(missing, ","))
        write_status(pawn, p.kit, false, p.tries, "verifying: missing " .. table.concat(missing, ","), false, false)
        p.verify_at, p.due = nil, now
    end
    if now < p.due then return end
    -- IO-1: a pawn that is already visible (safety reveal: the kit came late)
    -- waits until its hair has settled (main.lua dress_wait).
    local wait = api.dress_wait and api.dress_wait(pawn) or 0
    if wait > 0 then
        if not p.hair_note then
            p.hair_note = true
            Log("own kit %s: pawn is visible undressed; dressing in %.1f s (hair settle)", tostring(p.kit.class), wait)
        end
        p.due = now + wait
        return
    end
    -- The game's own armour setup may still be running; wait only briefly.
    if p.tries > 0 and api.busy(pawn) and now - p.due < BUSY_WAIT then return end
    p.tries = p.tries + 1
    pcall(apply_own, pawn, p.kit)
    p.verify_at = now + VERIFY_AFTER
end

-- World changed (round reset re-opens the map): the old pawn is gone; forget
-- it without touching it. The new pawn gets the kit through the normal path.
function Kit.on_world_change()
    own.key, own.pending, own.deferred, own.stable, own.watch, own.held = nil, nil, false, nil, nil, nil
    own.context = nil
    -- The next world's first status is always written (with a new
    -- "t"), even if it reads the same as the old world's: a pooled pawn FName
    -- comes back, and the Director keys its kit step on name + "t".
    status_sig = nil
end

-- Ctrl+F7 (with the gear dump): re-apply own kit now.
function Kit.force_local()
    own.key, own.pending, own.stable, own.watch, own.held = nil, nil, nil, nil, nil
    own.context = nil
    Log("own kit: forced re-apply requested")
end

return Kit

