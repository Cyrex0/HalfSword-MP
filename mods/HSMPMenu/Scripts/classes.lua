-- classes.lua — HSMPMenu "LOADOUT" sub-screen (classes / gear selection).
--
-- A native-menu sub-screen built with ui_kit.lua (no overlays, no cyclers,
-- no scrolling): class cards on the left, the equipment slots grouped by body
-- region in the centre, a paged item-picker grid with slot tabs and tier /
-- type filters on the right (illegal or over-budget items greyed), and a
-- footer with the points bar, the server verdict and SAVE & BACK.
-- Keyboard / gamepad: arrows move, Enter picks, Q/E (LB/RB) switch the slot
-- tab, PgUp/PgDn (LT/RT) page the grid, Esc goes back (asks first when the
-- kit has unsaved edits).
--
-- Data:
--   catalogue  HSMPLoadout/Scripts/hsmp_catalog.lua (real Half Sword classes)
--   records (schema loadout.rs; see "shared memory" below): slot "kit" (our choice,
--   sent by the sidecar until acked), "kit_rules_req" (host's rules request; the
--   lobby also sends it as a kit_rules command), "kit_rules" (server rules) and the
--   per-peer "peer_kit" (server-validated kits)
--
-- main.lua calls Classes.attach(api) once, then enter_screen("classes") uses
-- Classes.build as the screen builder. Everything runs on the game thread
-- (HSMPMenu's click poll).

local Classes = {}
local api, Cat, J
local msg = ""          -- one-line feedback on the message line
local sel               -- { class, r, l, armor = { group -> id }, cos = { face, hair, tint, 0 } }

local function Log(fmt, ...) if api then api.log("[classes] " .. fmt, ...) end end

-- --- catalogue ------------------------------------------------------------------

local function load_catalog()
    local tried = {}
    local function try(path)
        tried[#tried + 1] = path
        local ok, c = pcall(dofile, path)
        if ok and type(c) == "table" and c.items then return c end
        return nil
    end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", ""):gsub("\\", "/")
    local mods = src:match("^(.*)/HSMPMenu/Scripts/[^/]*$")
    local c = mods and try(mods .. "/HSMPLoadout/Scripts/hsmp_catalog.lua")
    c = c or try("ue4ss/Mods/HSMPLoadout/Scripts/hsmp_catalog.lua")
    c = c or try("Mods/HSMPLoadout/Scripts/hsmp_catalog.lua")
    if not c then
        local ok, r = pcall(require, "hsmp_catalog")
        if ok and type(r) == "table" and r.items then c = r end
    end
    if not c then Log("ERROR: hsmp_catalog.lua not found (tried %s)", table.concat(tried, " ; ")) end
    return c
end

-- --- shared memory: the loadout domain's records (schema loadout.rs) ----------------
--   slot "kit"            our selection (a `kit` record: seq, class, r, l, cos[4], rows =
--                         { {id} }); the sidecar sends it to the server until acked
--   slot "kit_rules_req"  the host's rules request (seq 0 = none)
--   slot "kit_rules"      the server's current rules (sidecar-written)
--   per-peer "peer_kit"   the server-validated kit of each peer (sidecar-written)

local function ipc() return rawget(_G, "HSMP_IPC") end


-- Armour ids of a kit record (its rows).
local function rec_armor(t)
    local out = {}
    for i, r in ipairs(type(t) == "table" and type(t.rows) == "table" and t.rows or {}) do out[i] = r.id end
    return out
end

-- --- selection <-> records --------------------------------------------------------------

local function armor_list(s)
    local out = {}
    for _, g in ipairs(Cat.groups) do
        local id = s.armor[g.id]
        if id and id ~= "" then out[#out + 1] = id end
    end
    return out
end

local function as_check(s)
    return { class = s.class, r = s.r or "", l = s.l or "", armor = armor_list(s) }
end

local function from_class(class_id, keep_cos)
    if class_id == "none" then
        return { class = "none", r = "", l = "", armor = {}, cos = keep_cos or { 0, 0, 0, 0 } }
    end
    local k = Cat.class_selection(class_id) or Cat.class_selection(Cat.default_class)
    local s = { class = k.class, r = k.r or "", l = k.l or "", armor = {}, cos = keep_cos or { 0, 0, 0, 0 } }
    for _, id in ipairs(k.armor) do
        local it = Cat.items[id]
        if it then s.armor[it.group] = id end
    end
    return s
end

local function load_choice()
    local I = ipc()
    local t = I and I.rec and I.rec("kit")   -- our own slot, read back
    if type(t) ~= "table" or type(t.class) ~= "string" or t.class == "" then return nil end
    if t.class ~= "none" and not Cat.class_by_id[t.class] then return nil end
    local s = { class = t.class, r = "", l = "", armor = {}, cos = { 0, 0, 0, 0 } }
    local function ok_weapon(id) local it = Cat.items[id]; return it and it.kind == "weapon" end
    if ok_weapon(t.r) then s.r = t.r end
    if ok_weapon(t.l) then s.l = t.l end
    for _, id in ipairs(rec_armor(t)) do
        local it = Cat.items[id]
        if it and it.kind == "armor" then s.armor[it.group] = id end
    end
    if type(t.cos) == "table" then
        for i = 1, 3 do s.cos[i] = math.max(0, math.min(7, math.floor(tonumber(t.cos[i]) or 0))) end
    end
    s.seq = tonumber(t.seq) or 0
    return s
end

local last_seq = 0
local function save_choice()
    local base = (os.time() - 1700000000)
    last_seq = math.max(last_seq + 1, base, (sel.seq or 0) + 1)
    sel.seq = last_seq
    local rows = {}
    for i, id in ipairs(armor_list(sel)) do rows[i] = { id = id } end
    local I = ipc()
    local ok, err = false, "no ipc"
    if I then
        ok, err = I.put("kit", { seq = sel.seq, class = sel.class, r = sel.r or "", l = sel.l or "",
            cos = { sel.cos[1], sel.cos[2], sel.cos[3], 0 }, rows = rows })
    end
    Log("saved kit seq=%d class=%s cost=%d -> %s", sel.seq, sel.class, Cat.kit_cost(as_check(sel)),
        ok and "kit slot" or ("WRITE FAILED: " .. tostring(err)))
    return ok and true or false
end

local function server_rules()
    local I = ipc()
    local t = I and I.rec and I.rec("kit_rules")
    if type(t) ~= "table" or (tonumber(t.rev) or 0) == 0 then return nil end
    return { mode = tonumber(t.mode) or 0, budget = tonumber(t.budget) or Cat.default_budget }
end

local function requested_rules()
    local I = ipc()
    local t = I and I.rec and I.rec("kit_rules_req")
    if type(t) ~= "table" or (tonumber(t.seq) or 0) == 0 then return nil end
    return { mode = tonumber(t.mode) or 0, budget = tonumber(t.budget) or Cat.default_budget }
end

local function rules_now()
    return server_rules() or { mode = Cat.MODE_FREE, budget = Cat.default_budget, unknown = true }
end

-- The host's rules request: the kit_rules_req slot (sent by the sidecar while we are
-- admin) and, unless the caller sends it itself, the kit_rules command (lobby path).
local function request_rules(mode, budget, file_only)
    local I = ipc()
    if I then I.put("kit_rules_req", { seq = os.time(), mode = mode, budget = budget }) end
    Log("host rules request: mode=%s budget=%d", Cat.mode_names[mode] or tostring(mode), budget)
    if not file_only and api.send_rules then pcall(api.send_rules, mode, budget) end
end

-- The server-validated kit of `pid` as { class, r, l, armor = {ids}, rev, verdict, reason }.
local function read_peer_kit(pid)
    if not pid or pid == 0 then return nil end
    local I = ipc()
    local t = I and I.peer_rec and I.peer_rec("peer_kit", pid)
    if type(t) ~= "table" or type(t.class) ~= "string" or t.class == "" then return nil end
    return { class = t.class, r = t.r, l = t.l, armor = rec_armor(t), rev = t.rev, verdict = t.verdict, reason = t.reason }
end

-- --- public helpers for the lobby ----------------------------------------------------------

local function class_label(id)
    if id == "none" then return "GAME GEAR" end
    local k = Cat and Cat.class_by_id[id]
    return k and k.label or tostring(id)
end

local function same_as_preset(t)
    local k = Cat.class_by_id[t.class]
    if not k then return true end
    if (t.r or "") ~= (k.r or "") or (t.l or "") ~= (k.l or "") then return false end
    local a, n = {}, 0
    for _, id in ipairs(k.armor) do a[id] = true; n = n + 1 end
    local m = 0
    for _, id in ipairs(type(t.armor) == "table" and t.armor or {}) do
        if not a[id] then return false end
        m = m + 1
    end
    return m == n
end

-- Lobby player-row class column: "KNIGHT", "KNIGHT+" (customised), "GAME GEAR", or "-".
function Classes.peer_class(pid)
    if not Cat then return "-" end
    local ok, t = pcall(read_peer_kit, pid)
    if not ok or not t then return "-" end
    local lbl = class_label(t.class)
    if t.class ~= "none" and not same_as_preset(t) then lbl = lbl .. "+" end
    return lbl
end

-- Our own saved choice, for the lobby: "DUELIST  11 PTS".
function Classes.my_summary()
    if not Cat then return "" end
    local s = load_choice()
    if not s then return "DEFAULT KIT" end
    if s.class == "none" then return "GAME GEAR" end
    local c = as_check(s)
    local lbl = class_label(s.class)
    if not same_as_preset(c) then lbl = "CUSTOM (" .. lbl .. ")" end
    return string.format("%s  %d PTS", lbl, Cat.kit_cost(c))
end

-- The server's kit rules { mode, budget } or nil; the budget presets.
function Classes.server_rules() if not Cat then return nil end; return server_rules() end
function Classes.budgets() return Cat and Cat.budgets or nil end
function Classes.request_rules(mode, budget, file_only) if Cat then request_rules(mode, budget, file_only) end end

-- --- UI ----------------------------------------------------------------------------------
--
--  +--------------------------------------------------------------------------------+
--  | LOADOUT                                                   RULES: CUSTOM 30     |
--  | CLASS CARDS        | EQUIPMENT                   | ITEM PICKER (tabs, filters, |
--  |  KNIGHT      42    | HEAD   HEAD   Armet      6 |  paged grid of tiles)       |
--  | [====== 23 / 30 POINTS ======]   server verdict                                |
--  | status line                                                                    |
--  | [RESET TO CLASS] [HOST RULES]                       [SAVE & BACK] [BACK]       |
--  | keys                                                        focused help       |
--  +--------------------------------------------------------------------------------+
--
-- Built once per screen entry; every click re-renders in place (labels,
-- colours, visibility). No cyclers, no scrolling.

local Kit                       -- ui_kit (api.kit)
local b                         -- current build (Kit.frame) - dropped on exit / world change
local ui = { slot = "head", tier = 0, wtype = "all", page = 1, view = "items" }
local msg_color
local saved_sig

local TIER_NAMES = { "I", "II", "III", "IV" }
local function tier_of(cost)
    cost = cost or 0
    if cost <= 1 then return 1 elseif cost <= 3 then return 2 elseif cost <= 5 then return 3 end
    return 4
end

local WTYPES = { { "all", "ALL" }, { "1h", "1H" }, { "2h", "2H" }, { "dagger", "DAGGER" }, { "shield", "SHIELD" } }
local WTYPE_LABEL = { ["1h"] = "ONE-HANDED", ["2h"] = "TWO-HANDED", dagger = "DAGGER", shield = "SHIELD" }

local ROLE = {
    knight      = "Full plate, longsword. Slow tank",
    man_at_arms = "Mail and poleaxe. All-rounder",
    duelist     = "Sword and buckler. Fast",
    brute       = "Great axe, padding. Heavy hitter",
    peasant     = "Rags and a pitchfork",
    custom      = "Your own kit within the rules",
    none        = "Keep the arena's own gear",
}
-- the same, for narrow cards (small windows)
local ROLE_SHORT = {
    knight      = "Plate, longsword. Tank",
    man_at_arms = "Mail, poleaxe. All-round",
    duelist     = "Sword, buckler. Fast",
    brute       = "Great axe. Heavy hitter",
    peasant     = "Rags, pitchfork",
    custom      = "Your own kit",
    none        = "The arena's gear",
}

-- Slot groups (the 15 game layers collapsed into body regions).
local REGION_OF = {
    head = "HEAD", bevor = "HEAD", collar = "HEAD",
    body = "TORSO", mail = "TORSO", chest = "TORSO", tabard = "TORSO", waist = "TORSO",
    shoulders = "ARMS", arms = "ARMS", hands = "ARMS",
    legs = "LEGS", thighs = "LEGS", shins = "LEGS", feet = "LEGS",
}
local REGIONS = { "HEAD", "TORSO", "ARMS", "LEGS" }
local COS = { face = 1, hair = 2, tint = 3 }
local COS_LABEL = { face = "FACE", hair = "HAIR", tint = "CLOTH TINT" }
local GROUPS                    -- built from the catalogue in attach()

local function build_groups()
    local by = {}
    for _, r in ipairs(REGIONS) do by[r] = { id = r, label = r, slots = {} } end
    for _, g in ipairs(Cat.groups) do
        table.insert(by[REGION_OF[g.id] or "TORSO"].slots, g.id)
    end
    GROUPS = {}
    for _, r in ipairs(REGIONS) do GROUPS[#GROUPS + 1] = by[r] end
    GROUPS[#GROUPS + 1] = { id = "WEAPONS", label = "WEAPONS", slots = { "R", "L" } }
    GROUPS[#GROUPS + 1] = { id = "LOOK", label = "LOOK", slots = { "face", "hair", "tint" }, collapsed = true }
end

local function group_of(slot)
    for _, g in ipairs(GROUPS) do
        for _, s in ipairs(g.slots) do if s == slot then return g end end
    end
    return GROUPS[1]
end

local function slot_label(slot)
    if slot == "R" then return "RIGHT HAND" elseif slot == "L" then return "LEFT HAND" end
    if COS[slot] then return COS_LABEL[slot] end
    for _, g in ipairs(Cat.groups) do if g.id == slot then return g.label end end
    return tostring(slot):upper()
end

local function is_weapon_slot(slot) return slot == "R" or slot == "L" end

local function slot_value(slot)
    if slot == "R" then return sel.r or "" elseif slot == "L" then return sel.l or "" end
    if COS[slot] then return sel.cos[COS[slot]] or 0 end
    return sel.armor[slot] or ""
end

local function cos_names(slot)
    if slot == "face" then return Cat.faces elseif slot == "hair" then return Cat.hair end
    return Cat.tints
end

local function cos_name(slot, idx)
    local n = cos_names(slot)[(idx or 0) + 1]
    if type(n) == "table" then n = n[1] end
    return tostring(n or "?")
end

local function sig(s)
    if not s then return "" end
    local a = armor_list(s); table.sort(a)
    return table.concat({ s.class, s.r or "", s.l or "", table.concat(a, ","),
        tostring(s.cos[1]), tostring(s.cos[2]), tostring(s.cos[3]) }, "|")
end

local function set_msg(text, color) msg = text or ""; msg_color = color end

local function admin_now()
    if api.is_host() then return true end
    local me = api.my_peer_id()
    -- the sidecar's link record (is_admin) and the server's admin_state record (slot `admin`)
    local ipc = rawget(_G, "HSMP_IPC")
    if not (ipc and ipc.rec) then return false end
    local link = ipc.rec("link")
    if type(link) == "table" and link.is_admin == true then return true end
    local ad = ipc.rec("admin")
    return me ~= 0 and type(ad) == "table" and ad.admin_peer == me
end

-- Why the kit cannot be edited right now (nil = editable).
local function edit_lock(rules)
    if rules.mode == Cat.MODE_CLASSES then return "CLASSES ONLY: the host locked kits - pick a class card" end
    if sel.class == "none" then return "GAME GEAR keeps the arena's gear - pick a class or CUSTOM first" end
    return nil
end

-- Kit cost if `id` went into `slot` (a two-hander also empties the left hand).
local function cost_with(slot, id)
    local c = as_check(sel)
    local cost = Cat.kit_cost(c)
    if COS[slot] then return cost end
    cost = cost - Cat.item_cost(slot_value(slot)) + Cat.item_cost(id)
    if slot == "R" then
        local it = Cat.items[id]
        if it and it.group == "2h" then cost = cost - Cat.item_cost(sel.l) end
    end
    return cost
end

-- Is `id` legal in `slot` now? Returns ok, short reason.
local function legal(slot, id, rules)
    if COS[slot] then return true, "" end             -- cosmetics are never locked
    local lock = edit_lock(rules)
    if lock then return false, (rules.mode == Cat.MODE_CLASSES) and "CLASSES ONLY" or "PICK A CLASS" end
    if id == "" then return true, "" end
    local it = Cat.items[id]
    if not it then return false, "UNKNOWN" end
    if slot == "R" and it.group == "shield" then return false, "LEFT HAND ONLY" end
    if slot == "L" and it.group == "2h" then return false, "RIGHT HAND ONLY" end
    if slot == "L" then
        local r = Cat.items[sel.r]
        if r and r.group == "2h" then return false, "2H IN RIGHT HAND" end
    end
    if rules.mode == Cat.MODE_CUSTOM then
        local c = cost_with(slot, id)
        if c > rules.budget then return false, string.format("OVER BUDGET %d/%d", c, rules.budget) end
    end
    return true, ""
end

-- Picker entries for the current slot + filters: list of ids ("" = NONE) or cosmetic indices.
local function candidates(rules)
    local slot = ui.slot
    local out = {}
    if COS[slot] then
        for i = 0, #cos_names(slot) - 1 do out[#out + 1] = i end
        return out
    end
    out[1] = ""
    local function tier_ok(id) return ui.tier == 0 or tier_of(Cat.item_cost(id)) == ui.tier end
    if is_weapon_slot(slot) then
        local good, bad = {}, {}
        for _, hg in ipairs(Cat.hands) do
            if ui.wtype == "all" or ui.wtype == hg then
                for _, id in ipairs(Cat.order[hg] or {}) do
                    if tier_ok(id) then
                        local hand_ok = not ((slot == "R" and hg == "shield") or (slot == "L" and hg == "2h"))
                        table.insert(hand_ok and good or bad, id)
                    end
                end
            end
        end
        for _, id in ipairs(good) do out[#out + 1] = id end
        for _, id in ipairs(bad) do out[#out + 1] = id end
    else
        for _, id in ipairs(Cat.order[slot] or {}) do
            if tier_ok(id) then out[#out + 1] = id end
        end
    end
    return out
end

local function equip(slot, id)
    if COS[slot] then
        sel.cos[COS[slot]] = id
        set_msg(slot_label(slot) .. ": " .. cos_name(slot, id))
        return
    end
    if slot == "R" then
        sel.r = id
        local it = Cat.items[id]
        if it and it.group == "2h" and (sel.l or "") ~= "" then
            sel.l = ""
            set_msg("Two-handed weapon: left hand emptied", Kit.C.ok)
            return
        end
    elseif slot == "L" then
        sel.l = id
    else
        sel.armor[slot] = (id ~= "") and id or nil
    end
    local it = Cat.items[id]
    set_msg(slot_label(slot) .. ": " .. (it and string.format("%s (%d pts)", it.label, it.cost) or "none"))
end

local function select_slot(slot)
    if ui.slot ~= slot then
        ui.slot = slot; ui.page = 1; ui.tier = 0; ui.wtype = "all"
    end
    ui.view = "items"
end

local function server_line()
    local mine = read_peer_kit(api.my_peer_id())
    if not mine then return "SERVER: no kit confirmed yet", Kit.C.dim, false end
    if (tonumber(mine.ack) or 0) < (sel.seq or 0) then return "SERVER: waiting for confirmation...", Kit.C.ok, true end
    if tonumber(mine.verdict) == 0 then return "SERVER: ACCEPTED (" .. class_label(mine.class) .. ")", Kit.C.good, false end
    return "SERVER REPLACED IT WITH " .. class_label(mine.class) .. ": " .. tostring(mine.reason), Kit.C.bad, false
end

local function rules_label(r)
    local s
    if r.mode == Cat.MODE_CUSTOM then s = "CUSTOM " .. tostring(r.budget)
    else s = Cat.mode_names[r.mode] or "?" end
    return "RULES: " .. s .. (r.unknown and "  (offline)" or "")
end

-- --- render ---------------------------------------------------------------------------------

function Classes.render()
    if not b or not Kit or not Kit.alive(b) or not sel then return end
    local w = b.w
    local rules = rules_now()
    local chk = as_check(sel)
    local cost = Cat.kit_cost(chk)
    local edited = sel.class ~= "none" and not same_as_preset(chk)
    local lock = (not COS[ui.slot]) and edit_lock(rules) or nil
    b.is_admin = admin_now()

    Kit.spinner_set(w.rules, false, rules_label(rules), rules.unknown and Kit.C.dim or Kit.C.head)

    -- class cards
    for _, c in ipairs(w.cards) do
        local id = c.id
        local shown = true
        local on, dis, cost_s, sub = false, false, "", ROLE[id] or ""
        if id == "none" then
            shown = rules.mode == Cat.MODE_FREE or sel.class == "none"
            on = sel.class == "none"
            dis = rules.mode ~= Cat.MODE_FREE
        elseif id == "custom" then
            on = edited
            dis = rules.mode == Cat.MODE_CLASSES
            cost_s = edited and (cost .. " PTS") or "--"
            if edited then sub = "Based on " .. class_label(sel.class) end
        else
            local kc = Cat.kit_cost(Cat.class_selection(id))
            cost_s = kc .. " PTS"
            on = (sel.class == id) and not edited
            dis = rules.mode == Cat.MODE_CUSTOM and kc > rules.budget
            if dis then sub = string.format("Over budget (%d > %d)", kc, rules.budget) end
        end
        Kit.show(c.ref, shown)
        if shown then
            Kit.set_state(c.ref, on, dis)
            Kit.set_text(c.cost, cost_s)
            Kit.set_text_fit(c.sub, { sub, sub == ROLE[id] and ROLE_SHORT[id] or nil })
            Kit.set_color(c.cost, on and Kit.C.on_text or (dis and Kit.C.dim or Kit.C.head))
            Kit.set_color(c.sub, on and Kit.C.on_text or (dis and Kit.C.bad or Kit.C.dim))
        end
    end

    -- slot list
    for _, r in ipairs(w.slots) do
        local slot = r.slot
        local name, pts = "", ""
        if r.collapsed then
            -- compact: the full names are on the FACE / HAIR / CLOTH TINT tabs
            if (sel.cos[1] or 0) == 0 and (sel.cos[2] or 0) == 0 and (sel.cos[3] or 0) == 0 then
                name = "GAME DEFAULT"
            else
                name = string.format("FACE %d  HAIR %d  TINT %d", sel.cos[1] or 0, sel.cos[2] or 0, sel.cos[3] or 0)
            end
        elseif COS[slot] then
            name = cos_name(slot, slot_value(slot))
        else
            local id = slot_value(slot)
            local it = id ~= "" and Cat.items[id] or nil
            name = it and it.label or "NONE"
            pts = it and tostring(it.cost) or ""
        end
        local on = (ui.view == "items") and ((slot == ui.slot) or (r.collapsed and COS[ui.slot] ~= nil))
        Kit.set_state(r.ref, on, false)
        Kit.set_text(r.name, name)
        Kit.set_text(r.pts, pts)
        local empty = (name == "NONE")
        Kit.set_color(r.name, on and Kit.C.on_text or (empty and Kit.C.dim or Kit.C.text))
        Kit.set_color(r.pts, on and Kit.C.on_text or Kit.C.head)
        Kit.set_color(r.label, on and Kit.C.on_text or Kit.C.dim)
    end

    -- right column: items picker or host rules
    local items_view = ui.view == "items"
    for _, ref in ipairs(w.picker_refs) do Kit.show(ref, items_view) end
    for _, t in ipairs(w.picker_texts) do Kit.show_text(t, items_view) end
    for _, ref in ipairs(w.rules_refs) do Kit.show(ref, not items_view) end
    for _, t in ipairs(w.rules_texts) do Kit.show_text(t, not items_view) end

    if items_view then
        local g = group_of(ui.slot)
        Kit.set_text(w.p_title, g.label .. "  >  " .. slot_label(ui.slot))
        -- tabs = the slots of this group
        for i, ref in ipairs(w.tabs) do
            local s = g.slots[i]
            Kit.show(ref, s ~= nil)
            if s then
                ref.slot_id = s
                Kit.set_label(ref, slot_label(s))
                Kit.set_state(ref, s == ui.slot, false)
            end
        end
        -- filters
        local cos = COS[ui.slot] ~= nil
        local weap = is_weapon_slot(ui.slot)
        for _, ref in ipairs(w.tier.chips) do Kit.show(ref, not cos) end
        Kit.show_text(w.tier_lbl, not cos)
        Kit.chip_group_set(w.tier, ui.tier, false)
        for _, ref in ipairs(w.wtype.chips) do Kit.show(ref, weap) end
        Kit.show_text(w.wtype_lbl, weap)
        Kit.chip_group_set(w.wtype, ui.wtype, false)

        local list = candidates(rules)
        local per = #w.tiles
        local page_items, pages, page = Kit.paginate(list, ui.page, per)
        ui.page = page
        local cur = slot_value(ui.slot)
        for i, ref in ipairs(w.tiles) do
            local v = page_items[i]
            ref.value = v
            if v == nil then
                Kit.tile_set(ref, nil)
            else
                local ok, why = legal(ui.slot, v, rules)
                local d = { on = (v == cur), disabled = not ok }
                if COS[ui.slot] then
                    d.title = cos_name(ui.slot, v)
                    d.meta = (v == 0) and "GAME DEFAULT" or "COSMETIC"
                    d.note = (v == cur) and "EQUIPPED" or ""
                elseif v == "" then
                    d.title = "NONE"
                    d.meta = "EMPTY  |  0 PTS"
                    d.note = (v == cur) and "EQUIPPED" or (ok and "" or why)
                else
                    local it = Cat.items[v]
                    d.title = it.label
                    d.meta = string.format("TIER %s  |  %d PTS", TIER_NAMES[tier_of(it.cost)], it.cost)
                    if v == cur then d.note = "EQUIPPED"
                    elseif not ok then d.note = why
                    elseif weap then d.note = WTYPE_LABEL[it.group] or ""
                    else d.note = "" end
                    if ok and rules.mode == Cat.MODE_CUSTOM and v ~= cur then
                        local delta = cost_with(ui.slot, v) - cost
                        if delta ~= 0 and d.note == "" then d.note = string.format("%+d PTS", delta) end
                    end
                end
                d.note_color = (not ok) and Kit.C.bad or nil
                ref.reason = (not ok) and (edit_lock(rules) or (slot_label(ui.slot) .. ": " .. why)) or nil
                Kit.tile_set(ref, d)
            end
        end
        Kit.pager_set(w.pager, page, pages)
        Kit.set_text(w.p_hint, lock or string.format("%d ITEMS  -  PAGE WITH PGUP / PGDN", #list))
        Kit.set_color(w.p_hint, lock and Kit.C.warn or Kit.C.dim)
    else
        local want = requested_rules() or rules
        Kit.chip_group_set(w.mode, want.mode, false)
        Kit.chip_group_set(w.budget, want.budget, want.mode ~= Cat.MODE_CUSTOM)
        Kit.set_text(w.r_info1, "REQUESTED: " .. rules_label(want):gsub("^RULES: ", ""))
        Kit.set_text(w.r_info2, "SERVER NOW: " .. rules_label(rules):gsub("^RULES: ", ""))
    end
    Kit.show(w.host_btn, b.is_admin)
    if w.host_btn then Kit.set_state(w.host_btn, not items_view, false) end

    -- footer
    local frac, col, lbl
    if rules.mode == Cat.MODE_CUSTOM then
        frac = cost / math.max(1, rules.budget)
        col = (cost > rules.budget) and Kit.C.bad or ((frac >= 0.9) and Kit.C.ok or Kit.C.good)
        lbl = string.format("%d / %d POINTS", cost, rules.budget)
    elseif rules.mode == Cat.MODE_CLASSES then
        frac, col, lbl = cost / 60, Kit.C.dim, string.format("%d POINTS  -  CLASS KITS ONLY", cost)
    else
        frac, col, lbl = cost / 60, Kit.C.ok, string.format("%d POINTS  -  NO LIMIT", cost)
    end
    if sel.class == "none" then frac, lbl = 0, "GAME GEAR  -  NO POINTS" end
    Kit.bar_set(w.bar, frac, col, lbl)

    local st, sc, busy = server_line()
    Kit.spinner_set(w.verdict, busy, st, sc)
    local ok, why = Cat.check(chk, rules.mode, rules.budget)
    local line, lc
    if msg ~= "" then line, lc = msg, msg_color or Kit.C.text
    elseif not ok then line, lc = "NOT ALLOWED: " .. why, Kit.C.bad
    elseif sig(sel) ~= saved_sig then line, lc = "UNSAVED CHANGES - SAVE & BACK keeps them", Kit.C.warn
    else line, lc = "KIT SAVED - VALID UNDER THE CURRENT RULES", Kit.C.good end
    Kit.msg(line, lc)
    Kit.set_state(w.save, false, not ok)
    w.save.reason = (not ok) and ("NOT ALLOWED: " .. tostring(why)) or nil
end

-- --- click handlers ---------------------------------------------------------------------------

local function after(fn)
    return function()
        if not b or not Kit.alive(b) then return end
        fn()
        Classes.render()
    end
end

local function pick_class(id)
    local rules = rules_now()
    if id == "custom" then
        if rules.mode == Cat.MODE_CLASSES then set_msg("CLASSES ONLY: the host locked custom kits", Kit.C.warn); return end
        if sel.class == "none" then sel = from_class(Cat.default_class, sel.cos) end
        set_msg("CUSTOM: pick a slot, then an item on the right (base: " .. class_label(sel.class) .. ")")
        return
    end
    sel = from_class(id, sel.cos)
    local k = Cat.class_by_id[id]
    set_msg(class_label(id) .. ": " .. (k and k.blurb or ROLE.none))
    Log("class card %s", id)
end

local function card_disabled_reason(id)
    local rules = rules_now()
    if id == "none" then return "GAME GEAR is only allowed under FREE rules" end
    if id == "custom" then return "CLASSES ONLY: the host locked custom kits" end
    local kc = Cat.kit_cost(Cat.class_selection(id))
    return string.format("%s costs %d points - over the %d point budget", class_label(id), kc, rules.budget)
end

local function click_tile(ref)
    local v = ref.value
    if v == nil then return end
    local rules = rules_now()
    local ok, why = legal(ui.slot, v, rules)
    if not ok then
        set_msg(edit_lock(rules) or (slot_label(ui.slot) .. ": " .. why), Kit.C.warn)
        return
    end
    equip(ui.slot, v)
end

-- Harness (HSMP_AUTOTEST `kit <class> [r=<id>] [l=<id>] [armor=<id,id,..>]`): the class card,
-- the slot edits a player makes under CUSTOM rules (armor= replaces the whole armour set), then
-- SAVE, without the screen. The same Cat.check as the SAVE button decides.
function Classes.autotest_kit(arg)
    local id = tostring(arg or ""):match("^(%S+)") or ""
    if id ~= "none" and not Cat.class_by_id[id] then return false, "unknown class " .. tostring(id) end
    sel = sel or load_choice() or from_class(Cat.default_class)
    sel = from_class(id, sel.cos)
    for k, v in tostring(arg):gmatch("(%a+)=(%S*)") do
        if k == "r" or k == "l" then
            if v ~= "" and not (Cat.items[v] and Cat.items[v].kind == "weapon") then return false, "not a weapon: " .. v end
            sel[k] = v
        elseif k == "armor" then
            sel.armor = {}
            for a in v:gmatch("[^,]+") do
                local it = Cat.items[a]
                if not (it and it.kind == "armor") then return false, "not armour: " .. a end
                sel.armor[it.group] = a
            end
        else
            return false, "unknown field " .. k
        end
    end
    local rules = rules_now()
    local good, reason = Cat.check(as_check(sel), rules.mode, rules.budget)
    if not good then return false, reason end
    if not save_choice() then return false, "cannot write the kit slot" end
    return true
end

local function do_save(and_back)
    local rules = rules_now()
    local good, reason = Cat.check(as_check(sel), rules.mode, rules.budget)
    if not good then set_msg("NOT SAVED: " .. reason, Kit.C.bad); return false end
    if not save_choice() then set_msg("SAVE FAILED: cannot write the kit slot", Kit.C.bad); return false end
    saved_sig = sig(sel)
    set_msg("SAVED - sent to the server; worn from your next spawn / round", Kit.C.good)
    if and_back then
        local back = api.back
        sel = nil; msg = ""
        back()
    end
    return true
end

-- BACK / Esc: unsaved edits are dropped only after a confirm.
local function go_back()
    if not b or not Kit.alive(b) then return end
    local function leave() set_msg(""); sel = nil; api.back() end
    if sel and sig(sel) ~= saved_sig then
        Kit.confirm("Discard your kit changes?", "DISCARD", leave, { no_label = "KEEP EDITING" })
        return
    end
    leave()
end

-- Q / E (LB / RB): the previous / next slot tab of the selected body region.
local function cycle_tab(d)
    if not b or not Kit.alive(b) or ui.view ~= "items" then return end
    local g = group_of(ui.slot)
    local i = 1
    for k, s in ipairs(g.slots) do if s == ui.slot then i = k end end
    select_slot(g.slots[((i - 1 + d) % #g.slots) + 1])
    Classes.render()
end

-- --- build ------------------------------------------------------------------------------------

local function build_unavailable()
    local L
    L, b = Kit.frame("classes", "LOADOUT UNAVAILABLE", { w = 1000, h = 520 })
    Kit.text("HSMPLoadout/Scripts/hsmp_catalog.lua is missing - reinstall HSMP.", L.x0, L.top, L.iw, L.u(40),
        L.F(Kit.TS.body), 0, Kit.C.text)
    Kit.actions(L, nil, { { label = "BACK", key = "back", cb = function() api.back() end } })
    b.on_back = function() api.back() end
    Kit.focus_default()
end

function Classes.build()
    Kit = api.kit
    if not Kit then Log("ERROR: ui_kit missing"); return end
    if not Cat or not J then build_unavailable(); return end
    sel = sel or load_choice() or from_class(Cat.default_class)
    saved_sig = sig(load_choice())
    if not GROUPS then build_groups() end
    if not group_of(ui.slot) then ui.slot = "head" end

    local L
    L, b = Kit.frame("classes", "LOADOUT", { hint_extra = { "tabs", "page" } })
    local u, F = L.u, L.F
    local s = L.s
    b.s = s
    local w = b.w
    w.cards, w.slots, w.tabs, w.tiles = {}, {}, {}, {}
    w.picker_refs, w.picker_texts, w.rules_refs, w.rules_texts = {}, {}, {}, {}
    w.rules = L.status
    w.status = L.msg

    -- columns inside the content area; the points bar sits at the bottom of it
    local gap = u(Kit.SP.lg)
    local iw = L.iw
    local bar_h = u(34)
    local top = L.top
    local bh = L.bottom - top - bar_h - u(Kit.SP.md)
    local lw = math.floor(iw * 0.21)
    local mw = math.floor(iw * 0.33)
    local rw = iw - lw - mw - 2 * gap
    local lx = L.x0
    local mx = lx + lw + gap
    local rx = mx + mw + gap
    b.layout = { px = L.px, py = L.py, pw = L.pw, ph = L.ph, lx = lx, lw = lw, mx = mx, mw = mw, rx = rx, rw = rw,
                 top = top, bh = bh }

    -- column headings
    local sh = u(30)
    Kit.text("CLASS", lx, top, lw, sh, F(Kit.TS.label), 0, Kit.C.dim)
    Kit.text("EQUIPMENT", mx, top, mw, sh, F(Kit.TS.label), 0, Kit.C.dim)
    local ctop = top + sh
    local cbh = bh - sh

    -- class cards
    Kit.group("class")
    local cards = {}
    for _, k in ipairs(Cat.classes) do cards[#cards + 1] = { id = k.id, label = k.label } end
    cards[#cards + 1] = { id = "custom", label = "CUSTOM" }
    cards[#cards + 1] = { id = "none", label = "GAME GEAR" }
    local cgap = u(Kit.SP.sm)
    local card_h = math.min(u(100), math.floor((cbh - (#cards - 1) * cgap) / #cards))
    for i, c in ipairs(cards) do
        local y = ctop + (i - 1) * (card_h + cgap)
        local id = c.id
        local ref = Kit.button("", after(function() pick_class(id) end), lx, y, lw, card_h,
            { style = "row", no_label = true, fkey = "classes:card:" .. id,
              help = (ROLE[id] or "") .. (id ~= "none" and id ~= "custom" and " - picks the whole kit" or "") })
        ref.on_disabled = after(function() set_msg(card_disabled_reason(id), Kit.C.warn) end)
        ref.reason = function() return card_disabled_reason(id) end
        local ip = u(Kit.SP.md)
        local nfs, sfs = F(Kit.TS.h2 + 1), F(Kit.TS.small)
        local name_h = math.ceil(nfs * Kit.LINE_H)
        local sub_h = math.ceil(sfs * Kit.LINE_H)
        local cost_w = math.floor(lw * 0.32)
        local name = Kit.text(c.label, lx + ip, y + math.floor(ip / 2), lw - 2 * ip - cost_w, name_h, nfs, 0, Kit.C.text, { z = 14 })
        local cost = Kit.text("", lx + lw - ip - cost_w, y + math.floor(ip / 2), cost_w, name_h, F(Kit.TS.label), 2, Kit.C.head, { z = 14 })
        -- the role line wraps onto a second line when the card is too narrow for it (small windows)
        local sub_lines = 1
        local role_len = 0
        for _, t in pairs(ROLE) do role_len = math.max(role_len, #t) end
        if role_len * sfs * Kit.CHAR_W > lw - 2 * ip and card_h - math.floor(ip / 2) * 2 - name_h >= 2 * sub_h then sub_lines = 2 end
        local sub = Kit.text("", lx + ip, y + card_h - math.floor(ip / 2) - sub_lines * sub_h, lw - 2 * ip, sub_lines * sub_h,
            sfs, 0, Kit.C.dim, { z = 14, wrap = sub_lines > 1 and sub_lines or nil })
        ref.label = name
        ref.texts = { name, cost, sub }
        w.cards[#w.cards + 1] = { id = id, ref = ref, name = name, cost = cost, sub = sub }
    end

    -- slot list (grouped by body region; LOOK collapses to one row)
    Kit.group("slots")
    local rows = 0
    for _, g in ipairs(GROUPS) do rows = rows + (g.collapsed and 1 or #g.slots) end
    local rgap, ggap = u(Kit.SP.xs), u(Kit.SP.sm)
    local glw = math.floor(mw * 0.20)
    local row_h = math.floor((cbh - (rows - #GROUPS) * rgap - (#GROUPS - 1) * ggap) / rows)
    row_h = math.min(row_h, u(46))
    local y = ctop
    local rfs = math.min(F(Kit.TS.label), math.floor(row_h / Kit.LINE_H))
    local lfs = math.min(F(Kit.TS.small), rfs)
    for _, g in ipairs(GROUPS) do
        Kit.text(g.label, mx, y, glw - u(Kit.SP.xs), row_h, lfs, 0, Kit.C.head)
        local list = g.collapsed and { g.slots[1] } or g.slots
        for si, slot in ipairs(list) do
            local bx, bw = mx + glw, mw - glw
            local target = slot
            local ref = Kit.button("", after(function() select_slot(target) end), bx, y, bw, row_h,
                { style = (si % 2 == 1) and "row" or "rowb", no_label = true, fkey = "classes:slot:" .. slot,
                  help = g.collapsed and "Face, hair and cloth tint (never locked)" or ("Choose your " .. slot_label(slot):lower()) })
            local ip = u(Kit.SP.sm)
            local llw = math.floor(bw * 0.34)
            local pw_ = math.floor(bw * 0.12)
            local label = Kit.text(g.collapsed and "APPEARANCE" or slot_label(slot), bx + ip, y, llw - ip, row_h,
                lfs, 0, Kit.C.dim, { z = 14 })
            local name = Kit.text("", bx + llw, y, bw - llw - pw_ - ip, row_h, rfs, 0, Kit.C.text, { z = 14 })
            local pts = Kit.text("", bx + bw - pw_ - ip, y, pw_, row_h, rfs, 2, Kit.C.head, { z = 14 })
            ref.label = name
            ref.texts = { label, name, pts }
            w.slots[#w.slots + 1] = { slot = slot, collapsed = g.collapsed, ref = ref, label = label, name = name, pts = pts }
            y = y + row_h + rgap
        end
        y = y - rgap + ggap
    end

    -- right column: item picker
    Kit.group("picker")
    local P_ = w.picker_refs
    local PT = w.picker_texts
    local cfs = F(Kit.TS.small)
    local ch_ = u(Kit.BH.tab)
    local vg = u(Kit.SP.sm)
    local py2 = top
    w.p_title = Kit.text("", rx, py2, rw, sh, F(Kit.TS.h2), 0, Kit.C.head); PT[#PT + 1] = w.p_title
    py2 = py2 + sh + vg
    local max_tabs = 0
    for _, g in ipairs(GROUPS) do max_tabs = math.max(max_tabs, #g.slots) end
    local tgap = u(6)
    local tw = math.floor((rw - (max_tabs - 1) * tgap) / max_tabs)
    for i = 1, max_tabs do
        local ref
        ref = Kit.button("", after(function() if ref.slot_id then select_slot(ref.slot_id) end end),
            rx + (i - 1) * (tw + tgap), py2, tw, ch_, { fs = cfs, style = "head", fkey = "classes:tab" .. i,
            help = "Slot tab (Q / E switch)" })
        w.tabs[i] = ref; P_[#P_ + 1] = ref
    end
    py2 = py2 + ch_ + vg
    local flw = math.floor(rw * 0.14)
    w.tier_lbl = Kit.text("TIER", rx, py2, flw, ch_, cfs, 0, Kit.C.dim); PT[#PT + 1] = w.tier_lbl
    local tiers = { { 0, "ALL" } }
    for i, n in ipairs(TIER_NAMES) do tiers[#tiers + 1] = { i, n } end
    w.tier = Kit.chip_group(tiers, rx + flw, py2, rw - flw, ch_,
        function(k) after(function() ui.tier = k; ui.page = 1 end)() end,
        { fs = cfs, gap = tgap, help = "Filter by cost: I 0-1, II 2-3, III 4-5, IV 6+ points" })
    for _, r in ipairs(w.tier.chips) do P_[#P_ + 1] = r end
    py2 = py2 + ch_ + vg
    w.wtype_lbl = Kit.text("TYPE", rx, py2, flw, ch_, cfs, 0, Kit.C.dim); PT[#PT + 1] = w.wtype_lbl
    w.wtype = Kit.chip_group(WTYPES, rx + flw, py2, rw - flw, ch_,
        function(k) after(function() ui.wtype = k; ui.page = 1 end)() end, { fs = cfs, gap = tgap, help = "Filter weapons by type" })
    for _, r in ipairs(w.wtype.chips) do P_[#P_ + 1] = r end
    py2 = py2 + ch_ + vg
    -- grid
    local pager_h = u(38)
    local hint_h = u(26)
    local grid_h = top + bh - py2 - pager_h - hint_h - 2 * vg
    local cols = 4
    local grows = (grid_h >= 4 * u(104)) and 4 or 3
    local rects = Kit.grid_rects(rx, py2, rw, grid_h, cols, grows, u(Kit.SP.sm))
    for i, r in ipairs(rects) do
        local ref
        ref = Kit.tile(after(function() click_tile(ref) end), r[1], r[2], r[3], r[4],
            { scale = s, fs = F(Kit.TS.label), fkey = "classes:tile" .. i, help = "Equip this item" })
        ref.on_disabled = ref.entry and after(function() click_tile(ref) end) or nil
        w.tiles[i] = ref; P_[#P_ + 1] = ref
    end
    b.grid = { cols = cols, rows = grows }
    py2 = py2 + grid_h + vg
    w.p_hint = Kit.text("", rx, py2, rw, hint_h, cfs, 0, Kit.C.dim); PT[#PT + 1] = w.p_hint
    py2 = py2 + hint_h + vg
    w.pager = Kit.pager(rx, py2, rw, pager_h,
        after(function() ui.page = math.max(1, ui.page - 1) end),
        after(function() ui.page = ui.page + 1 end), { fs = F(Kit.TS.label), bw = u(160) })
    P_[#P_ + 1] = w.pager.prev; P_[#P_ + 1] = w.pager.next; PT[#PT + 1] = w.pager.label

    -- right column: host rules view (same area, hidden unless HOST RULES is on)
    Kit.group("rules")
    local R_, RT = w.rules_refs, w.rules_texts
    local ry = top
    RT[#RT + 1] = Kit.text("MATCH RULES (HOST)", rx, ry, rw, sh, F(Kit.TS.h2), 0, Kit.C.head)
    ry = ry + sh + vg * 2
    RT[#RT + 1] = Kit.text("MODE", rx, ry, rw, sh, F(Kit.TS.small), 0, Kit.C.dim)
    ry = ry + sh
    local chip_h = u(Kit.BH.field)
    local modes = { { Cat.MODE_FREE, "FREE" }, { Cat.MODE_CLASSES, "CLASSES ONLY" }, { Cat.MODE_CUSTOM, "CUSTOM" } }
    w.mode = Kit.chip_group(modes, rx, ry, rw, chip_h, function(k)
        after(function()
            local want = requested_rules() or rules_now()
            request_rules(k, want.budget or Cat.default_budget)
            set_msg("Rules request sent - applies at the next spawn / round", Kit.C.ok)
        end)()
    end, { fs = F(Kit.TS.label), gap = tgap, help = "Which kits the server allows" })
    for _, r in ipairs(w.mode.chips) do R_[#R_ + 1] = r end
    ry = ry + chip_h + vg * 2
    RT[#RT + 1] = Kit.text("POINT BUDGET (CUSTOM)", rx, ry, rw, sh, F(Kit.TS.small), 0, Kit.C.dim)
    ry = ry + sh
    local budgets = {}
    for _, v in ipairs(Cat.budgets) do budgets[#budgets + 1] = { v, tostring(v) } end
    w.budget = Kit.chip_group(budgets, rx, ry, rw, chip_h, function(k)
        after(function()
            request_rules(Cat.MODE_CUSTOM, k)
            set_msg("Budget request sent: CUSTOM " .. k, Kit.C.ok)
        end)()
    end, { fs = F(Kit.TS.label), gap = tgap, help = "Points every CUSTOM kit must fit in" })
    for _, r in ipairs(w.budget.chips) do
        R_[#R_ + 1] = r
        r.reason = "The budget only applies to CUSTOM - pick CUSTOM first"
        r.on_disabled = after(function() set_msg("Budget only applies in CUSTOM mode - pick CUSTOM first", Kit.C.warn) end)
    end
    ry = ry + chip_h + vg * 3
    w.r_info1 = Kit.text("", rx, ry, rw, sh, F(Kit.TS.label), 0, Kit.C.text); RT[#RT + 1] = w.r_info1
    ry = ry + sh
    w.r_info2 = Kit.text("", rx, ry, rw, sh, F(Kit.TS.label), 0, Kit.C.text); RT[#RT + 1] = w.r_info2
    ry = ry + sh
    RT[#RT + 1] = Kit.text("Every player's kit is re-checked by the server.", rx, ry, rw, sh, F(Kit.TS.small), 0, Kit.C.dim)
    ry = ry + sh
    RT[#RT + 1] = Kit.text("New rules apply at the next spawn / round.", rx, ry, rw, sh, F(Kit.TS.small), 0, Kit.C.dim)
    ry = ry + sh + vg * 2
    local back_items = Kit.button("BACK TO ITEMS", after(function() ui.view = "items" end), rx, ry,
        math.floor(math.min(rw, u(260))), chip_h, { fs = F(Kit.TS.label), help = "Back to the item picker" })
    R_[#R_ + 1] = back_items

    -- footer: points bar + server verdict (bottom of the content), then the actions
    local fy = L.bottom - bar_h
    w.bar = Kit.bar(lx, fy, lw + gap + mw, bar_h, F(Kit.TS.label))
    w.verdict = Kit.spinner(rx, fy, rw, bar_h, F(Kit.TS.label), 0, Kit.C.dim)
    local acts = Kit.actions(L,
        { { label = "RESET TO CLASS", key = "reset", help = "Back to the selected class's own kit", cb = after(function()
              if sel.class == "none" then set_msg("GAME GEAR has no preset", Kit.C.warn); return end
              sel = from_class(sel.class, sel.cos)
              set_msg("Kit reset to the " .. class_label(sel.class) .. " preset")
          end) },
          { label = "HOST RULES", key = "host", help = "Kit rules for this server (host only)", cb = after(function()
              ui.view = (ui.view == "rules") and "items" or "rules"
          end) } },
        { { label = "BACK", key = "back", cb = go_back, help = "Back to the lobby (asks before dropping unsaved changes)" },
          { label = "SAVE & BACK", key = "save", style = "primary", help = "Save the kit and send it to the server", cb = function()
              if not b or not Kit.alive(b) then return end
              if not do_save(true) then Classes.render() end
          end } })
    w.reset, w.host_btn, w.back, w.save = acts.reset, acts.host, acts.back, acts.save
    w.save.on_disabled = after(function()
        local rules = rules_now()
        local _, why = Cat.check(as_check(sel), rules.mode, rules.budget)
        set_msg("NOT SAVED: " .. tostring(why), Kit.C.bad)
    end)

    -- keys: Esc = BACK, Q/E = slot tab, PgUp/PgDn = picker page
    b.on_back = go_back
    b.on_tab = cycle_tab
    b.on_page = function(d)
        if ui.view ~= "items" then return end
        ui.page = math.max(1, ui.page + d)
        Classes.render()
    end

    -- live refresh (server verdict, rules) once a second while on screen
    Kit.on_tick(20, function() Classes.render() end)

    Log("loadout build: canvas %.0fx%.0f scale %.2f panel %dx%d grid %dx%d widgets=%d",
        L.cw, L.ch, s, L.pw, L.ph, cols, grows, b.n)
    Classes.render()
    Kit.focus_default(w.slots[1] and w.slots[1].ref)
end

-- Screen exit / world change: drop every widget reference without touching it.
function Classes.forget()
    b = nil
end

-- api: { log, state_dir, kit, json, viewport() -> vw, vh, refresh(), back(), is_host(), my_peer_id(),
--        send_rules(mode, budget) }
function Classes.attach(a)
    api = a
    Kit = a.kit
    J = a.json
    if not J then
        local ok, m = pcall(require, "jsonlite")
        if ok and type(m) == "table" then J = m end
    end
    Cat = load_catalog()
    if Cat then
        local n = 0; for _ in pairs(Cat.items) do n = n + 1 end
        Log("catalogue: %d items, %d classes", n, #Cat.classes)
        build_groups()
    end
end

-- exposed for offline tests
Classes._ui = ui
Classes._legal = function(slot, id) return legal(slot, id, rules_now()) end
Classes._sel = function() return sel end
Classes._build_ref = function() return b end
Classes._candidates = function() return candidates(rules_now()) end

return Classes
