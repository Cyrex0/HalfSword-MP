-- hsmp_cfg.lua -- install-relative HSMP configuration (replaces hard-coded D:/ paths).
--
-- build-and-deploy.ps1 copies this file into every mod's Scripts/ directory;
-- do not edit the per-mod copies.
--
-- File format: hsmp.cfg, plain "key = value" lines, '#' or ';' comments,
-- blank lines ignored, values may be wrapped in double quotes. Unknown keys
-- are kept (cfg.get) so new keys need no library change.
--
--     # hsmp.cfg (next to HalfswordUE5-Win64-Shipping.exe)
--     bin_dir    = hsmp                       # relative to the game's Win64 dir, or absolute
--     master_url = https://master.halfswordmp.workers.dev   # the public list (the default)
--     # dev / test deploys write master_url = http://127.0.0.1:7778 (a local master)
--     # or a list: primary first, then fallbacks (comma-separated)
--     # master_url = https://master.example.net, https://backup.example.net:7778
--
-- Lookup order for the file (first that exists wins):
--   1. $HSMP_CFG                      (absolute or relative to the game's cwd)
--   2. hsmp.cfg                       (cwd = ...\Binaries\Win64 when the game runs)
--   3. ue4ss/hsmp.cfg
-- Then environment overrides (dev and test harness):
--   HSMP_BIN_DIR, HSMP_MASTER_URL, HSMP_SERVER_EXE, HSMP_SIDECAR_EXE, HSMP_QUERY_EXE
--
-- API (all three access forms agree; values are normalised):
--   local cfg = require("hsmp_cfg")    -- or dofile next to main.lua
--   cfg.bin_dir, cfg.master_url        -> plain string fields (filled at load time)
--   cfg.get(key [, default])           -> string value or default (any key)
--   cfg.load()[key]                    -> same values from the table (cfg.load(true) re-reads)
--   cfg.exe(name)                      -> bin_dir .. "/" .. name .. ".exe"  (name = "hsmp-server")
--   cfg.server_exe(), cfg.sidecar_exe(), cfg.query_exe()   (HSMP_*_EXE env still override)
--   cfg.state_dir()                    -> HSMP_STATE_DIR or "hsmp_state" (forward slashes)
--   cfg.mods_cache_dir()               -> HSMP_MODS_CACHE or "hsmp_mods": the server-mods cache
--                                         (docs/hosting/server-mods.md; outside ue4ss/Mods)
--   cfg.inst()                         -> HSMP_INST or "0"
--   cfg.dev()                          -> true when HSMP_DEV=1 (dev keys / dev panels allowed)
--   cfg.source()                       -> path of the cfg file actually read (or nil)
--   cfg.safe_url(u)                    -> u if it is a cmd-safe http(s) URL, else nil
--   cfg.master_urls()                  -> { primary, fallback, ... } (a copy; never empty)
-- Normalisation: bin_dir uses forward slashes without a trailing slash;
-- master_url must pass safe_url() (it ends up inside os.execute command
-- lines), otherwise the default is used. A comma-separated master_url keeps
-- every safe entry (duplicates dropped) in cfg.master_urls(); cfg.master_url
-- is the first of them (the one a hosted server registers with).

local M = {}

M.DEFAULTS = {
    bin_dir    = "hsmp",
    master_url = "https://master.halfswordmp.workers.dev",
}

local cache, cache_src = nil, nil

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local function env(k) local v = trim(os.getenv(k)); if v == "" then return nil end; return v end

function M.parse(text)
    local t = {}
    for line in (text or ""):gmatch("[^\r\n]+") do
        local l = trim(line)
        if l ~= "" and not l:match("^[#;]") then
            local k, v = l:match("^([%w_%.%-]+)%s*=%s*(.-)$")
            if k then
                v = v:gsub("%s+[#;].*$", "")             -- trailing comment
                v = trim(v):gsub('^"(.*)"$', "%1")
                t[k:lower()] = v
            end
        end
    end
    return t
end

local function read_file(p)
    local f = p and io.open(p, "rb")
    if not f then return nil end
    local s = f:read("*a"); f:close()
    return s
end

function M.candidates()
    local list = {}
    local e = env("HSMP_CFG")
    if e then list[#list + 1] = e end
    list[#list + 1] = "hsmp.cfg"
    list[#list + 1] = "ue4ss/hsmp.cfg"
    return list
end

-- Only plain http(s)://host[:port][/path] with cmd-safe characters: the URL
-- ends up inside os.execute command lines.
function M.safe_url(u)
    u = trim(u or "")
    if u and u:match("^https?://[%w%.%-]+[:%d]*[%w%./%-_]*$") then return (u:gsub("/+$", "")) end
    return nil
end

-- "a, b ,c" -> { safe(a), safe(b), safe(c) } (unsafe entries and duplicates dropped).
-- Only commas separate: a value with a space inside one URL stays one (unsafe) entry.
function M.split_urls(s)
    local out, seen = {}, {}
    for part in (tostring(s or "") .. ","):gmatch("([^,]*),") do
        local u = M.safe_url(part)
        if u and not seen[u] and #out < 8 then seen[u] = true; out[#out + 1] = u end
    end
    return out
end

function M.master_urls()
    M.load()
    local o = {}
    for i, u in ipairs(M._urls or { M.DEFAULTS.master_url }) do o[i] = u end
    return o
end

function M.load(force)
    if cache and not force then return cache end
    local t, src = {}, nil
    for _, p in ipairs(M.candidates()) do
        local s = read_file(p)
        if s then t, src = M.parse(s), p; break end
    end
    for k, v in pairs(M.DEFAULTS) do if t[k] == nil or t[k] == "" then t[k] = v end end
    t.bin_dir    = env("HSMP_BIN_DIR") or t.bin_dir
    t.master_url = env("HSMP_MASTER_URL") or t.master_url
    t.bin_dir    = (t.bin_dir:gsub("\\", "/"):gsub("/+$", ""))
    if t.bin_dir == "" then t.bin_dir = M.DEFAULTS.bin_dir end
    local urls = M.split_urls(t.master_url)
    if #urls == 0 then urls = { M.DEFAULTS.master_url } end
    t.master_url = urls[1]
    t.master_urls = table.concat(urls, ",")
    M._urls = urls
    cache, cache_src = t, src
    M.bin_dir, M.master_url = t.bin_dir, t.master_url   -- plain field access: cfg.bin_dir
    return t
end

function M.source() M.load(); return cache_src end

function M.get(key, default)
    local v = M.load()[tostring(key):lower()]
    if v == nil or v == "" then return default end
    return v
end

function M.exe(name)
    return M.load().bin_dir .. "/" .. name .. ".exe"
end

function M.server_exe()  return ((env("HSMP_SERVER_EXE")  or M.exe("hsmp-server")):gsub("\\", "/")) end
function M.sidecar_exe() return ((env("HSMP_SIDECAR_EXE") or M.exe("hsmp-sidecar")):gsub("\\", "/")) end
function M.query_exe()   return ((env("HSMP_QUERY_EXE")   or M.exe("hsmp-query")):gsub("\\", "/")) end

function M.state_dir()
    return ((env("HSMP_STATE_DIR") or "hsmp_state"):gsub("\\", "/"))
end

-- The verified server-mods cache: the sidecar writes it (--mods-cache), HSMPModHost loads
-- from it. Relative to the game's Win64 folder unless absolute; never under ue4ss/Mods.
function M.mods_cache_dir()
    return ((env("HSMP_MODS_CACHE") or "hsmp_mods"):gsub("\\", "/"):gsub("/+$", ""))
end

function M.inst() return env("HSMP_INST") or "0" end
function M.dev() return os.getenv("HSMP_DEV") == "1" end

-- Test hook: forget the cache and re-read.
function M._reset() cache, cache_src = nil, nil; M.load() end

M.load()
return M
