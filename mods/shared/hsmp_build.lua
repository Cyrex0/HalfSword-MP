-- shared/hsmp_build.lua: the build identity of these mod files and how it compares.
--
-- Deploy and release write hsmp_build_id.lua next to every mod's main.lua (version,
-- protocol range, IPC ABI and layout, content hash; see crates/hsmp-net/src/build). The
-- sidecar prints its own with `hsmp-sidecar --build-info`. HSMPMenu compares the two at
-- startup and refuses to start multiplayer on a mismatch; the server browser uses the
-- same identity to grey out servers this install cannot join.
--
-- Pure logic: no UE4SS calls, so it runs in the offline tests.

local B = {}

B.MSG_OUTDATED = "Mod files are out of date, run the launcher to update"
B.MSG_MISSING = "Mod files are incomplete, run the launcher to repair"

local function num(v) return tonumber(v) or 0 end

-- The mods' identity table, or nil, reason. `loader(name)` returns a module table
-- (HSMPMenu's load_module).
function B.load(loader)
    local ok, t = pcall(loader, "hsmp_build_id")
    if not ok or type(t) ~= "table" then return nil, "hsmp_build_id.lua missing" end
    if type(t.version) ~= "string" or t.version == "" or type(t.content_hash) ~= "string" then
        return nil, "hsmp_build_id.lua malformed"
    end
    return t
end

-- `hsmp-sidecar --build-info` output (one JSON object) -> identity table, or nil.
function B.parse_info(text)
    if type(text) ~= "string" then return nil end
    local function s(k) return text:match('"' .. k .. '"%s*:%s*"([^"]*)"') end
    local function n(k) return tonumber(text:match('"' .. k .. '"%s*:%s*(%d+)')) end
    local t = {
        version = s("version"), content_hash = s("content_hash"), ipc_layout = s("ipc_layout"),
        protocol = n("protocol"), proto_min = n("proto_min"), proto_max = n("proto_max"),
        ipc_abi_major = n("ipc_abi_major"), ipc_abi_minor = n("ipc_abi_minor"),
    }
    if not t.version or not t.content_hash or not t.protocol then return nil end
    return t
end

local function range(t)
    local lo, hi = num(t.proto_min), num(t.proto_max)
    if lo == 0 then lo = num(t.protocol) end
    if hi == 0 then hi = num(t.protocol) end
    return lo, hi
end

-- Mods vs a component (the sidecar): nil when they match, else
-- { msg = <player text>, fields = {...}, detail = <log text> }.
function B.check(mine, theirs)
    if type(mine) ~= "table" then return { msg = B.MSG_MISSING, fields = { "mods" }, detail = "no hsmp_build_id.lua" } end
    if type(theirs) ~= "table" then return { msg = B.MSG_MISSING, fields = { "sidecar" }, detail = "no build info from hsmp-sidecar" } end
    local f, d = {}, {}
    local function bad(name, a, b) f[#f + 1] = name; d[#d + 1] = string.format("%s mods=%s sidecar=%s", name, tostring(a), tostring(b)) end
    if mine.version ~= theirs.version then bad("version", mine.version, theirs.version) end
    local alo, ahi = range(mine)
    local blo, bhi = range(theirs)
    if ahi < blo or bhi < alo then bad("protocol", alo .. ".." .. ahi, blo .. ".." .. bhi) end
    if num(mine.ipc_abi_major) ~= num(theirs.ipc_abi_major) or tostring(mine.ipc_layout):lower() ~= tostring(theirs.ipc_layout):lower() then
        bad("ipc", tostring(mine.ipc_abi_major) .. "/" .. tostring(mine.ipc_layout), tostring(theirs.ipc_abi_major) .. "/" .. tostring(theirs.ipc_layout))
    end
    if tostring(mine.content_hash):lower() ~= tostring(theirs.content_hash):lower() then
        bad("content", tostring(mine.content_hash):sub(1, 16), tostring(theirs.content_hash):sub(1, 16))
    end
    if #f == 0 then return nil end
    return { msg = B.MSG_OUTDATED, fields = f, detail = table.concat(d, "; ") }
end

-- The 16-hex tag the browser compares (query replies carry only this much).
function B.content_tag(id)
    return (type(id) == "table" and type(id.content_hash) == "string") and id.content_hash:sub(1, 16):lower() or ""
end

-- Can this install join server row `s` ({proto, proto_min, proto_max, content, version})?
-- true, or false + a short note ("needs v0.2.0"). Unknown fields never block.
function B.server_compat(mine, s)
    if type(mine) ~= "table" or type(s) ~= "table" then return true end
    local slo, shi = num(s.proto_min), num(s.proto_max)
    if slo == 0 then slo = num(s.proto) end
    if shi == 0 then shi = num(s.proto) end
    local mlo, mhi = range(mine)
    local proto_ok = slo == 0 or mlo == 0 or not (shi < mlo or mhi < slo)
    local tag = tostring(s.content or ""):lower()
    local content_ok = tag == "" or B.content_tag(mine) == "" or tag == B.content_tag(mine)
    if proto_ok and content_ok then return true end
    local v = tostring(s.version or "")
    if v ~= "" and v ~= mine.version then return false, "needs v" .. v end
    if not proto_ok then return false, string.format("needs protocol v%d", shi) end
    return false, "different mod files"
end

return B
