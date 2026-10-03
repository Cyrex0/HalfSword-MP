-- shared/hsmp_build.lua offline tests: the mods' build identity against the sidecar's
-- (`hsmp-sidecar --build-info`) and against server-browser rows.
--
--   hsmp-tools lua-test build_id
--
-- HSMPMenu's use of it (startup check, HOST / JOIN refused with the launcher hint) is
-- covered by menu_ui proc.3.

local B = dofile(T.path("mods/shared/hsmp_build.lua"))

local HASH = string.rep("ab", 32)
local function id(over)
    local t = { version = "0.1.0-beta.1", protocol = 6, proto_min = 6, proto_max = 6, ipc_abi_major = 2,
                ipc_abi_minor = 0, ipc_layout = "eb9a00a6a80b45ec", content_hash = HASH }
    for k, v in pairs(over or {}) do t[k] = v end
    return t
end
local function info_json(t)
    return string.format('{"product":"HalfSword-MP","version":"%s","protocol":%d,"proto_min":%d,"proto_max":%d,"ipc_abi_major":%d,"ipc_abi_minor":%d,"ipc_layout":"%s","content_hash":"%s"}\n',
        t.version, t.protocol, t.proto_min, t.proto_max, t.ipc_abi_major, t.ipc_abi_minor, t.ipc_layout, t.content_hash)
end
local function has(list, v) for _, x in ipairs(list or {}) do if x == v then return true end end return false end

T.log("== loading the mods' identity")
do
    local got = B.load(function(n) return n == "hsmp_build_id" and id() or nil end)
    T.check(type(got) == "table" and got.version == "0.1.0-beta.1", "hsmp_build_id is loaded through the mod's loader")
    local none, why = B.load(function() return nil end)
    T.check(none == nil and why:find("missing", 1, true), "no file -> nil, missing")
    local bad, why2 = B.load(function() return { version = "" } end)
    T.check(bad == nil and why2:find("malformed", 1, true), "a broken file -> nil, malformed")
    T.check(B.load(function() error("boom") end) == nil, "a loader error is caught")
end

T.log("== the sidecar's --build-info")
do
    local p = B.parse_info(info_json(id()))
    T.check(p and p.version == "0.1.0-beta.1" and p.protocol == 6 and p.proto_max == 6 and p.content_hash == HASH
        and p.ipc_layout == "eb9a00a6a80b45ec" and p.ipc_abi_major == 2, "every field parsed")
    T.check(B.parse_info("error: unexpected argument '--build-info' found\n") == nil, "an older sidecar's clap error -> nil")
    T.check(B.parse_info(nil) == nil and B.parse_info("") == nil, "no output -> nil")
end

T.log("== mods vs sidecar")
do
    T.check(B.check(id(), B.parse_info(info_json(id()))) == nil, "same build -> nil")
    T.check(B.check(id(), id({ ipc_abi_minor = 3 })) == nil, "an IPC minor bump is compatible")
    T.check(B.check(id({ ipc_layout = "EB9A00A6A80B45EC" }), id()) == nil, "hex case does not matter")
    local r = B.check(id(), id({ version = "0.1.0-beta.2" }))
    T.check(r and r.msg == "Mod files are out of date, run the launcher to update" and has(r.fields, "version")
        and r.detail:find("version mods=0.1.0-beta.1 sidecar=0.1.0-beta.2", 1, true), "version mismatch -> the launcher message + detail")
    r = B.check(id(), id({ content_hash = string.rep("cd", 32) }))
    T.check(r and r.msg == B.MSG_OUTDATED and #r.fields == 1 and r.fields[1] == "content", "content mismatch alone")
    r = B.check(id(), id({ protocol = 7, proto_min = 7, proto_max = 7 }))
    T.check(r and has(r.fields, "protocol"), "no common protocol")
    r = B.check(id(), id({ ipc_layout = "0000000000000001" }))
    T.check(r and has(r.fields, "ipc"), "IPC layout mismatch")
    r = B.check(id(), id({ ipc_abi_major = 3 }))
    T.check(r and has(r.fields, "ipc"), "IPC ABI major mismatch")
    r = B.check(id(), nil)
    T.check(r and r.msg == "Mod files are incomplete, run the launcher to repair", "no sidecar answer -> repair")
    r = B.check(nil, id())
    T.check(r and r.msg == B.MSG_MISSING and has(r.fields, "mods"), "no mods identity -> repair")
end

T.log("== server browser rows")
do
    local mine = id()
    T.check(B.content_tag(mine) == "abababababababab", "content tag = first 16 hex")
    T.check(B.server_compat(mine, { proto = 6, content = "abababababababab", version = "0.1.0-beta.1" }) == true, "same build -> joinable")
    T.check(B.server_compat(mine, { proto = 6, content = "", version = "0.1.0-beta.1" }) == true, "no advertised hash -> joinable")
    T.check(B.server_compat(mine, { proto = 0, content = "" }) == true, "an old listing with nothing -> joinable (the handshake decides)")
    T.check(B.server_compat(mine, { proto = 6, proto_min = 5, proto_max = 7 }) == true, "overlapping range -> joinable")
    local ok, note = B.server_compat(mine, { proto = 6, content = "cdcdcdcdcdcdcdcd", version = "0.2.0" })
    T.check(ok == false and note == "needs v0.2.0", "other content + version -> needs v0.2.0 (" .. tostring(note) .. ")")
    ok, note = B.server_compat(mine, { proto = 5, version = "" })
    T.check(ok == false and note == "needs protocol v5", "older protocol, no version -> needs protocol v5 (" .. tostring(note) .. ")")
    ok, note = B.server_compat(mine, { proto = 6, content = "cdcdcdcdcdcdcdcd", version = "0.1.0-beta.1" })
    T.check(ok == false and note == "different mod files", "same version, other files")
    T.check(B.server_compat(nil, { proto = 5 }) == true, "no identity of our own -> nothing greyed")
end
