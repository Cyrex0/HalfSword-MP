-- HSMPMenu / jsonlite.lua — a small, tolerant JSON reader + flat-object writer
-- for the state the menu reads (shared-memory contracts through
-- shared/hsmp_ipc.lua, and real files such as .settings.json).
--
--   J.decode(s)      -> value | nil       (nil on any syntax error; never raises)
--   J.read(path)     -> value | nil
--   J.str(s)         -> a JSON string literal (ASCII; control chars escaped)
--   J.flat(t, order) -> '{"k":v,...}' for a flat table of strings / numbers / bools,
--                       keys in `order` first, then the rest sorted (stable output)
--   J.write_atomic(path, text) -> bool    (readers never see a half-written value)
--
-- Numbers that do not fit a double exactly (u64 epochs) should be compared as
-- the digit strings J.raw_int finds, never via tonumber.

local J = {}

function J.decode(s)
    if type(s) ~= "string" then return nil end
    local pos = 1
    local function ws() pos = s:find("[^%s]", pos) or (#s + 1) end
    local val
    local function str()
        local out, i = {}, pos + 1
        while i <= #s do
            local c = s:sub(i, i)
            if c == '"' then pos = i + 1; return table.concat(out) end
            if c == "\\" then
                local n = s:sub(i + 1, i + 1)
                if n == "u" then
                    local code = tonumber(s:sub(i + 2, i + 5), 16) or 63
                    out[#out + 1] = (code < 128) and string.char(code) or "?"
                    i = i + 6
                else
                    out[#out + 1] = ({ n = "\n", t = "\t", r = "\r", b = "\b", f = "\f" })[n] or n
                    i = i + 2
                end
            else
                out[#out + 1] = c; i = i + 1
            end
        end
        error("unterminated string")
    end
    function val()
        ws()
        local c = s:sub(pos, pos)
        if c == "{" then
            local o = {}; pos = pos + 1; ws()
            if s:sub(pos, pos) == "}" then pos = pos + 1; return o end
            while true do
                ws()
                if s:sub(pos, pos) ~= '"' then error("expected key") end
                local k = str(); ws()
                if s:sub(pos, pos) ~= ":" then error("expected :") end
                pos = pos + 1
                o[k] = val(); ws()
                local d = s:sub(pos, pos); pos = pos + 1
                if d == "}" then return o elseif d ~= "," then error("expected , or }") end
            end
        elseif c == "[" then
            local a, n = {}, 0; pos = pos + 1; ws()
            if s:sub(pos, pos) == "]" then pos = pos + 1; return a end
            while true do
                n = n + 1
                a[n] = val(); ws()           -- explicit index: a null keeps its slot
                local d = s:sub(pos, pos); pos = pos + 1
                if d == "]" then return a elseif d ~= "," then error("expected , or ]") end
            end
        elseif c == '"' then return str()
        elseif s:sub(pos, pos + 3) == "true" then pos = pos + 4; return true
        elseif s:sub(pos, pos + 4) == "false" then pos = pos + 5; return false
        elseif s:sub(pos, pos + 3) == "null" then pos = pos + 4; return nil
        else
            local num = s:match("^-?[%d%.eE+-]+", pos)
            if not num then error("bad value") end
            pos = pos + #num
            return tonumber(num)
        end
    end
    local ok, r = pcall(val)
    if ok then return r end
    return nil
end

-- A contract (shared memory) or a real file (.settings.json), decoded.
function J.read(path)
    local ipc = rawget(_G, "HSMP_IPC")   -- shared/hsmp_ipc.lua
    local s = ipc and ipc.read(path)
    if not s then return nil end
    return J.decode(s)
end

-- The raw digits of an integer field ("epoch":18446744073709551615 -> "18446744073709551615").
function J.raw_int(s, key)
    if type(s) ~= "string" then return nil end
    return s:match('"' .. key .. '"%s*:%s*"?(%d+)')
end

function J.str(s)
    return '"' .. tostring(s or ""):gsub("[\128-\255]", "?"):gsub('[%c"\\]', function(ch)
        if ch == '"' then return '\\"' elseif ch == "\\" then return "\\\\" end
        return string.format("\\u%04x", ch:byte())
    end) .. '"'
end

local function scalar(v)
    local t = type(v)
    if t == "string" then return J.str(v)
    elseif t == "boolean" then return v and "true" or "false"
    elseif t == "number" then
        if v ~= v or v == math.huge or v == -math.huge then return "0" end
        if math.type and math.type(v) == "integer" then return tostring(v) end
        if v == math.floor(v) and math.abs(v) < 2^53 then return string.format("%d", v) end
        return string.format("%.6g", v)
    end
    return nil
end

function J.flat(t, order)
    local parts, done = {}, {}
    for _, k in ipairs(order or {}) do
        local s = scalar(t[k])
        if s then parts[#parts + 1] = J.str(k) .. ":" .. s end
        done[k] = true
    end
    local rest = {}
    for k in pairs(t) do if type(k) == "string" and not done[k] then rest[#rest + 1] = k end end
    table.sort(rest)
    for _, k in ipairs(rest) do
        local s = scalar(t[k])
        if s then parts[#parts + 1] = J.str(k) .. ":" .. s end
    end
    return "{" .. table.concat(parts, ",") .. "}"
end

-- A contract (shared memory: codec slot, bus key, G2S request) or a real file
-- (.settings.json: tmp + rename), through shared/hsmp_ipc.lua.
function J.write_atomic(path, text)
    local ipc = rawget(_G, "HSMP_IPC")
    if not ipc then return false end
    return ipc.write(path, text .. "\n") and true or false
end

return J
