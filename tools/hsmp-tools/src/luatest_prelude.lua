-- hsmp-tools luatest prelude: pure-Lua helpers on the T table (see src/luatest.rs).
local T = T

-- Report (not hide) a call to an undefined global that a pcall / xpcall swallows
-- (the suite fails with it in luatest::run_suite). A suite that expects one on purpose passes
-- a function that raises it itself: error("...") is not reported.
do
    local raw_pcall, raw_xpcall, note = pcall, xpcall, T._nil_global_call
    local function seen(err)
        if type(err) == "string" and err:find("attempt to call a nil value (global '", 1, true) then note(err) end
    end
    pcall = function(f, ...)
        local r = table.pack(raw_pcall(f, ...))
        if not r[1] then seen(r[2]) end
        return table.unpack(r, 1, r.n)
    end
    xpcall = function(f, h, ...)
        return raw_xpcall(f, function(e) seen(e); return h(e) end, ...)
    end
end

-- Python truthiness (nil/false/0/"" are false; tables are true).
function T.truthy(v)
    return not (v == nil or v == false or v == 0 or v == "")
end

-- Shallow-copied, sorted list (default `<`; mixed types sort by tostring).
function T.sorted(t, cmp)
    local c = {}
    for i = 1, #t do c[i] = t[i] end
    table.sort(c, cmp or function(a, b)
        if type(a) == type(b) and (type(a) == "number" or type(a) == "string") then return a < b end
        return tostring(a) < tostring(b)
    end)
    return c
end

-- Deep equality.
function T.eq(a, b)
    if a == b then return true end
    if type(a) ~= "table" or type(b) ~= "table" then return false end
    for k, v in pairs(a) do if not T.eq(v, b[k]) then return false end end
    for k in pairs(b) do if a[k] == nil then return false end end
    return true
end

-- Short printable form for failure details.
function T.repr(v, depth)
    depth = depth or 2
    if type(v) == "string" then return string.format("%q", v) end
    if type(v) ~= "table" then return tostring(v) end
    if depth <= 0 then return "{...}" end
    local parts, n = {}, 0
    for i = 1, #v do
        n = n + 1; if n > 12 then parts[#parts + 1] = "..."; break end
        parts[#parts + 1] = T.repr(v[i], depth - 1)
    end
    for k, x in pairs(v) do
        if not (math.type(k) == "integer" and k >= 1 and k <= #v) then
            n = n + 1; if n > 12 then parts[#parts + 1] = "..."; break end
            parts[#parts + 1] = tostring(k) .. "=" .. T.repr(x, depth - 1)
        end
    end
    return "{" .. table.concat(parts, ", ") .. "}"
end

-- Plain (non-pattern) substring helpers.
function T.contains(s, sub) return s ~= nil and string.find(s, sub, 1, true) ~= nil end
function T.startswith(s, p) return s ~= nil and string.sub(s, 1, #p) == p end
function T.count(s, sub)
    local n, i = 0, 1
    if s == nil or sub == "" then return 0 end
    while true do
        local a, b = string.find(s, sub, i, true)
        if not a then return n end
        n = n + 1; i = b + 1
    end
end
-- Python str.splitlines() (\n, \r\n).
function T.lines(s)
    local out = {}
    if s == nil or s == "" then return out end
    for l in (s:gsub("\r\n", "\n") .. (s:sub(-1) == "\n" and "" or "\n")):gmatch("(.-)\n") do out[#out + 1] = l end
    return out
end

function T.keys(t) local o = {}; for k in pairs(t) do o[#o + 1] = k end; return o end
function T.map(t, f) local o = {}; for i = 1, #t do o[i] = f(t[i], i) end; return o end
function T.filter(t, f) local o = {}; for i = 1, #t do if f(t[i], i) then o[#o + 1] = t[i] end end; return o end
function T.any(t, f) for i = 1, #t do if f(t[i], i) then return true end end; return false end
function T.all(t, f) for i = 1, #t do if not f(t[i], i) then return false end end; return true end
function T.set(t) local o = {}; for i = 1, #t do o[t[i]] = true end; return o end
function T.len(t) local n = 0; for _ in pairs(t) do n = n + 1 end; return n end
-- Lua list of a lua table's 1..#t (nil-safe).
function T.list(t) local o = {}; if t == nil then return o end; for i = 1, #t do o[i] = t[i] end; return o end

-- The mods talk to the sidecar through HSMPNative (shared memory) only. Every
-- Lua state gets the file-backed mock (lua-tests/lib/hsmp_native_filemock.lua) as
-- its HSMPNative, so the suites can describe the sidecar as legacy state-dir
-- files. A suite that wants another binding sets HSMPNative (or passes
-- IPC.init{native = ...}) before it loads a mod.
do
    local ok, FM = pcall(require, "hsmp_native_filemock")
    if ok and type(FM) == "table" then
        HSMPNative = FM.new()
    else
        T.log("luatest prelude: hsmp_native_filemock unavailable: " .. tostring(FM))
    end
end
