-- The runtime role is fixed when a Lua state starts, before any mod registers
-- hooks or starts a loop. Unknown roles refuse both execution paths.
local M = {}
function M.parse(value)
    if value == nil or value == "" or value == "client" then return "client" end
    if value == "native_worker" then return "native_worker" end
    if value == "native_client" then return "native_client" end
    return "invalid"
end
local role = M.parse(os.getenv("HSMP_RUNTIME_ROLE"))
function M.name() return role end
function M.client() return role == "client" end
function M.worker() return role == "native_worker" end
function M.presentation() return role == "native_client" end
return M
