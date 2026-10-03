// Offline harness for HSMPNative (HSMP-SHM). Plays the part of UE4SS:
//
//  * owns the "host" Lua 5.4.7 copy (lua_vm_host, original UE4SS luauser.c), like UE4SS.dll;
//  * mode F: loads main.dll the way CppMod does (LoadLibraryExW + start_mod) with a mock
//    UE4SS.dll next to this exe, and dispatches lifecycle calls through RAW vtable offsets
//    taken from the real UE4SS.dll disassembly (hsmp_abi::kVt*), so a vtable drift in
//    ue4ss_abi.hpp fails this test; fires on_lua_start for a non-HSMP mod and three HSMP mods;
//  * mode E: no main.dll; each Lua state gets the table from
//    package.loadlib(<hsmp_lua.dll>, "luaopen_hsmp_lua"), as the facade does;
//  * runs harness_ipc.lua in the first HSMP state, with helpers to run code in the other
//    states and to spawn / kill (by PID) the fake sidecar process.
//
// usage: harness F <main.dll> <hsmp_lua.dll> <hsmp-fake-sidecar.exe> <harness_ipc.lua>
//        harness E <hsmp_lua.dll> <hsmp-fake-sidecar.exe> <harness_ipc.lua>
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include <cstdio>
#include <string>
#include <string_view>
#include <vector>

#include "lua.hpp"

#include "ue4ss_abi.hpp"

namespace
{
    struct FakeLua // layout-compatible with LuaMadeSimple::Lua's first member
    {
        lua_State* L;
    };

    using FnVoid = void (*)(void*);
    using FnLuaStartNoName = void (*)(void*, FakeLua&, FakeLua&, FakeLua&, FakeLua*);
    using FnLuaStartNamed = void (*)(void*, std::wstring_view, FakeLua&, FakeLua&, FakeLua&, FakeLua*);
    using FnLuaStartNoNameDep = void (*)(void*, FakeLua&, FakeLua&, FakeLua&, std::vector<FakeLua*>&);
    using FnLuaStartNamedDep = void (*)(void*, std::wstring_view, FakeLua&, FakeLua&, FakeLua&, std::vector<FakeLua*>&);

    template <typename Fn>
    Fn slot(void* obj, std::size_t offset)
    {
        void** vtable = *static_cast<void***>(obj);
        return reinterpret_cast<Fn>(vtable[offset / sizeof(void*)]);
    }

    // Mirrors LuaMod::fire_on_lua_start_for_cpp_mods for one C++ mod (named overloads).
    void fire_on_lua_start(void* mod, std::wstring_view lua_mod_name, lua_State* L)
    {
        FakeLua lua{L}, main_lua{L}, async_lua{L}, hook_lua{L};
        std::vector<FakeLua*> hooks{&hook_lua};
        slot<FnLuaStartNamed>(mod, hsmp_abi::kVtOnLuaStartNamed)(mod, lua_mod_name, lua, main_lua, async_lua, &hook_lua);
        slot<FnLuaStartNamedDep>(mod, hsmp_abi::kVtOnLuaStartNamedDeprecated)(mod, lua_mod_name, lua, main_lua, async_lua, hooks);
    }

    int fail(const char* what)
    {
        std::fprintf(stderr, "HARNESS FAIL: %s\n", what);
        return 1;
    }

    std::string utf8(const wchar_t* w)
    {
        int n = WideCharToMultiByte(CP_UTF8, 0, w, -1, nullptr, 0, nullptr, nullptr);
        std::string s(static_cast<size_t>(n > 0 ? n - 1 : 0), '\0');
        WideCharToMultiByte(CP_UTF8, 0, w, -1, s.data(), n, nullptr, nullptr);
        for (char& c : s)
            if (c == '\\') c = '/';
        return s;
    }

    std::vector<lua_State*> g_states; // [0] = non-HSMP mod (F) / unused (E), [1..3] = HSMP mods

    // run_in(i, code) -> string result of the chunk (or "ERROR: ...")
    int l_run_in(lua_State* L)
    {
        lua_Integer i = luaL_checkinteger(L, 1);
        const char* code = luaL_checkstring(L, 2);
        if (i < 0 || i >= static_cast<lua_Integer>(g_states.size()) || !g_states[i]) return luaL_error(L, "bad state index");
        lua_State* T = g_states[static_cast<size_t>(i)];
        int top = lua_gettop(T);
        if (luaL_loadstring(T, code) != LUA_OK || lua_pcall(T, 0, 1, 0) != LUA_OK)
        {
            std::string err = std::string("ERROR: ") + lua_tostring(T, -1);
            lua_settop(T, top);
            lua_pushstring(L, err.c_str());
            return 1;
        }
        std::string r = luaL_tolstring(T, -1, nullptr);
        lua_settop(T, top);
        lua_pushstring(L, r.c_str());
        return 1;
    }

    // spawn(cmdline) -> pid
    int l_spawn(lua_State* L)
    {
        std::string cmd = luaL_checkstring(L, 1);
        STARTUPINFOA si{};
        si.cb = sizeof si;
        PROCESS_INFORMATION pi{};
        std::vector<char> buf(cmd.begin(), cmd.end());
        buf.push_back('\0');
        if (!CreateProcessA(nullptr, buf.data(), nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi))
            return luaL_error(L, "CreateProcess failed: %lu", GetLastError());
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        lua_pushinteger(L, static_cast<lua_Integer>(pi.dwProcessId));
        return 1;
    }

    // kill(pid) -> true when terminated (by PID, never by name)
    int l_kill(lua_State* L)
    {
        DWORD pid = static_cast<DWORD>(luaL_checkinteger(L, 1));
        HANDLE h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, FALSE, pid);
        bool ok = false;
        if (h)
        {
            ok = TerminateProcess(h, 9) != 0;
            WaitForSingleObject(h, 5000);
            CloseHandle(h);
        }
        lua_pushboolean(L, ok);
        return 1;
    }

    // Native sampling / servo: the mock UE4SS.dll's fake world (F only; absent in E).
    HMODULE mock_dll()
    {
        return GetModuleHandleW(L"UE4SS.dll");
    }
    // mock_world() -> mesh, pawn, weapon, hand_r FName id, ProcessEvent calls so far
    int l_mock_world(lua_State* L)
    {
        auto f = reinterpret_cast<void (*)(uint64_t*)>(GetProcAddress(mock_dll(), "mock_world"));
        if (!f) return luaL_error(L, "mock_world not exported");
        uint64_t out[6]{};
        f(out);
        for (int i = 0; i < 6; ++i) lua_pushinteger(L, static_cast<lua_Integer>(out[i]));
        return 6;
    }
    // mock_fname(s) -> the mock's FName id of s
    int l_mock_fname(lua_State* L)
    {
        auto f = reinterpret_cast<uint32_t (*)(const wchar_t*)>(GetProcAddress(mock_dll(), "mock_fname"));
        if (!f) return luaL_error(L, "mock_fname not exported");
        std::string s = luaL_checkstring(L, 1);
        std::wstring w(s.begin(), s.end());
        lua_pushinteger(L, static_cast<lua_Integer>(f(w.c_str())));
        return 1;
    }
    // mock_kill(addr): the object's serial changes (weak pointers stop resolving)
    int l_mock_kill(lua_State* L)
    {
        auto f = reinterpret_cast<void (*)(uint64_t, int)>(GetProcAddress(mock_dll(), "mock_kill"));
        if (!f) return luaL_error(L, "mock_kill not exported");
        f(static_cast<uint64_t>(luaL_checkinteger(L, 1)), lua_isnoneornil(L, 2) ? 1 : lua_toboolean(L, 2));
        return 0;
    }
    // now_us() -> QPC microseconds
    int l_now_us(lua_State* L)
    {
        LARGE_INTEGER c, f;
        QueryPerformanceCounter(&c);
        QueryPerformanceFrequency(&f);
        lua_pushnumber(L, static_cast<lua_Number>(c.QuadPart) * 1e6 / static_cast<lua_Number>(f.QuadPart));
        return 1;
    }

    int l_sleep_ms(lua_State* L)
    {
        Sleep(static_cast<DWORD>(luaL_checkinteger(L, 1)));
        return 0;
    }

    lua_State* new_host_state(const char* mode, const std::string& e_dll, const std::string& fake)
    {
        lua_State* L = luaL_newstate();
        luaL_openlibs(L);
        lua_pushstring(L, mode);
        lua_setglobal(L, "MODE");
        lua_pushstring(L, e_dll.c_str());
        lua_setglobal(L, "HSMP_LUA_DLL");
        lua_pushstring(L, fake.c_str());
        lua_setglobal(L, "FAKE_SIDECAR");
        lua_pushinteger(L, static_cast<lua_Integer>(GetCurrentProcessId()));
        lua_setglobal(L, "GAME_PID");
        lua_register(L, "run_in", l_run_in);
        lua_register(L, "spawn", l_spawn);
        lua_register(L, "kill", l_kill);
        lua_register(L, "sleep_ms", l_sleep_ms);
        lua_register(L, "mock_world", l_mock_world);
        lua_register(L, "mock_fname", l_mock_fname);
        lua_register(L, "mock_kill", l_mock_kill);
        lua_register(L, "now_us", l_now_us);
        return L;
    }

    int run_script(lua_State* L, const std::string& script)
    {
        lua_getglobal(L, "debug");
        lua_getfield(L, -1, "traceback");
        lua_remove(L, -2);
        int msgh = lua_gettop(L);
        if (luaL_loadfile(L, script.c_str()) != LUA_OK)
        {
            std::fprintf(stderr, "%s\n", lua_tostring(L, -1));
            return fail("could not load the test script");
        }
        if (lua_pcall(L, 0, 0, msgh) != LUA_OK)
        {
            std::fprintf(stderr, "%s\n", lua_tostring(L, -1));
            return fail("test script raised an error");
        }
        lua_settop(L, 0);
        return 0;
    }
} // namespace

int wmain(int argc, wchar_t** argv)
{
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    if (argc < 2) return fail("usage: harness F|E ...");
    const std::wstring mode = argv[1];
    static const wchar_t* kMods[] = {L"HSMPSync", L"HSMPAvatars", L"HSMPHud"};

    if (mode == L"F")
    {
        if (argc < 6) return fail("usage: harness F <main.dll> <hsmp_lua.dll> <fake-sidecar.exe> <script>");
        const std::string e_dll = utf8(argv[3]), fake = utf8(argv[4]), script = utf8(argv[5]);
        SetEnvironmentVariableW(L"HSMPNATIVE_ALLOW_UNPINNED", L"1"); // mock UE4SS.dll is not the pinned build
        HMODULE dll = LoadLibraryExW(argv[2], nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (!dll)
        {
            std::fprintf(stderr, "LoadLibraryExW error %lu\n", GetLastError());
            return fail("could not load main.dll (missing/mismatched UE4SS.dll export?)");
        }
        auto start_mod = reinterpret_cast<void* (*)()>(GetProcAddress(dll, "start_mod"));
        auto uninstall_mod = reinterpret_cast<void (*)(void*)>(GetProcAddress(dll, "uninstall_mod"));
        if (!start_mod || !uninstall_mod) return fail("start_mod/uninstall_mod not exported");
        void* mod = start_mod();
        if (!mod) return fail("start_mod returned null");
        slot<FnVoid>(mod, hsmp_abi::kVtOnProgramStart)(mod);
        slot<FnVoid>(mod, hsmp_abi::kVtOnUnrealInit)(mod);
        slot<FnVoid>(mod, hsmp_abi::kVtOnUiInit)(mod);
        slot<FnVoid>(mod, hsmp_abi::kVtOnUpdate)(mod);

        lua_State* other = new_host_state("F", e_dll, fake);
        fire_on_lua_start(mod, L"SomeOtherMod", other);
        if (lua_getglobal(other, "HSMPNative") != LUA_TNIL) return fail("HSMPNative leaked into a non-HSMP mod");
        lua_pop(other, 1);
        g_states.push_back(other);
        for (const wchar_t* m : kMods)
        {
            lua_State* L = new_host_state("F", e_dll, fake);
            fire_on_lua_start(mod, m, L);
            if (lua_getglobal(L, "HSMPNative") != LUA_TTABLE) return fail("HSMPNative global missing after on_lua_start");
            lua_pop(L, 1);
            g_states.push_back(L);
        }
        std::printf("[harness] F: main.dll registered HSMPNative into 3 HSMP mods (raw vtable dispatch), not into SomeOtherMod\n");
        int rc = run_script(g_states[1], script);
        for (lua_State* L : g_states) lua_close(L);
        uninstall_mod(mod);
        FreeLibrary(dll);
        if (rc == 0) std::printf("[harness] F PASS\n");
        return rc;
    }
    if (mode == L"E")
    {
        if (argc < 5) return fail("usage: harness E <hsmp_lua.dll> <fake-sidecar.exe> <script>");
        const std::string e_dll = utf8(argv[2]), fake = utf8(argv[3]), script = utf8(argv[4]);
        g_states.push_back(nullptr);
        for (int i = 0; i < 3; ++i)
        {
            lua_State* L = new_host_state("E", e_dll, fake);
            std::string code = "HSMPNative = assert(package.loadlib('" + e_dll + "', 'luaopen_hsmp_lua'))()";
            if (luaL_dostring(L, code.c_str()) != LUA_OK)
            {
                std::fprintf(stderr, "%s\n", lua_tostring(L, -1));
                return fail("package.loadlib(hsmp_lua.dll) failed");
            }
            g_states.push_back(L);
        }
        std::printf("[harness] E: hsmp_lua.dll loaded into 3 Lua states via package.loadlib\n");
        int rc = run_script(g_states[1], script);
        for (size_t i = 1; i < g_states.size(); ++i) lua_close(g_states[i]);
        if (rc == 0) std::printf("[harness] E PASS\n");
        return rc;
    }
    return fail("mode must be F or E");
}
