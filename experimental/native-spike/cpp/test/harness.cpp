// Offline harness: plays the part of UE4SS for the transport spike.
//
//  * owns the "host" Lua 5.4.7 copy (lua_vm_host, original UE4SS luauser.c),
//    exactly like UE4SS.dll owns its own statically linked Lua;
//  * loads main.dll the way CppMod does (LoadLibraryExW + start_mod), with a
//    mock UE4SS.dll next to this exe providing the imported symbols;
//  * dispatches lifecycle calls through RAW vtable offsets taken from the real
//    UE4SS.dll disassembly (hsmp_abi::kVt*), not through the replica header,
//    so a vtable-order drift in ue4ss_abi.hpp makes this test fail;
//  * runs harness_test.lua, which exercises option F (HSMPNative global) and
//    option E (require "hsmp_lua") including the UDP loopback through the
//    Rust net thread.
//
// usage: harness <main.dll> <dir containing hsmp_lua.dll> <harness_test.lua>
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

    // Mirrors LuaMod::fire_on_lua_start_for_cpp_mods for one C++ mod.
    void fire_on_lua_start(void* mod, std::wstring_view lua_mod_name, bool same_name_as_cpp_mod, lua_State* L)
    {
        FakeLua lua{L}, main_lua{L}, async_lua{L}, hook_lua{L};
        std::vector<FakeLua*> hooks{&hook_lua};
        if (same_name_as_cpp_mod)
        {
            slot<FnLuaStartNoName>(mod, hsmp_abi::kVtOnLuaStartNoName)(mod, lua, main_lua, async_lua, &hook_lua);
            slot<FnLuaStartNoNameDep>(mod, hsmp_abi::kVtOnLuaStartNoNameDeprecated)(mod, lua, main_lua, async_lua, hooks);
        }
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

    lua_State* new_host_state(const std::string& cpath_dir)
    {
        lua_State* L = luaL_newstate();
        luaL_openlibs(L);
        lua_getglobal(L, "package");
        std::string cpath = cpath_dir + "/?.dll";
        lua_pushstring(L, cpath.c_str());
        lua_setfield(L, -2, "cpath");
        lua_pop(L, 1);
        return L;
    }
} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc < 4) return fail("usage: harness <main.dll> <hsmp_lua dir> <harness_test.lua>");
    std::setvbuf(stdout, nullptr, _IONBF, 0);

    const std::string cpath_dir = utf8(argv[2]);
    const std::string script = utf8(argv[3]);

    HMODULE dll = LoadLibraryExW(argv[1], nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
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
    std::printf("[harness] start_mod ok, mod=%p\n", mod);

    // lifecycle in UE4SS order
    slot<FnVoid>(mod, hsmp_abi::kVtOnProgramStart)(mod);
    slot<FnVoid>(mod, hsmp_abi::kVtOnUnrealInit)(mod);
    slot<FnVoid>(mod, hsmp_abi::kVtOnUiInit)(mod);
    slot<FnVoid>(mod, hsmp_abi::kVtOnUpdate)(mod);

    // A non-HSMP Lua mod must NOT get the table.
    lua_State* other = new_host_state(cpath_dir);
    fire_on_lua_start(mod, L"SomeOtherMod", false, other);
    if (lua_getglobal(other, "HSMPNative") != LUA_TNIL) return fail("HSMPNative leaked into a non-HSMP mod");
    lua_pop(other, 1);

    // The probe mod gets it (named overload at vtable +0x30).
    lua_State* L = new_host_state(cpath_dir);
    fire_on_lua_start(mod, L"HSMPNativeProbe", false, L);
    if (lua_getglobal(L, "HSMPNative") != LUA_TTABLE) return fail("HSMPNative global missing after on_lua_start (+0x30)");
    lua_pop(L, 1);
    std::printf("[harness] on_lua_start dispatched via raw vtable offsets; HSMPNative registered\n");

    // run the Lua test with a traceback handler
    lua_getglobal(L, "debug");
    lua_getfield(L, -1, "traceback");
    lua_remove(L, -2);
    int msgh = lua_gettop(L);
    if (luaL_loadfile(L, script.c_str()) != LUA_OK)
    {
        std::fprintf(stderr, "%s\n", lua_tostring(L, -1));
        return fail("could not load harness_test.lua");
    }
    if (lua_pcall(L, 0, 0, msgh) != LUA_OK)
    {
        std::fprintf(stderr, "%s\n", lua_tostring(L, -1));
        return fail("harness_test.lua raised an error");
    }
    lua_settop(L, 0);

    // teardown in UE4SS order: Lua states close (hsmp_lua.dll's CLIBS __gc runs
    // FreeLibrary; the module pinned itself so its code stays mapped), then
    // the C++ mod is uninstalled.
    lua_close(other);
    lua_close(L);
    uninstall_mod(mod);
    FreeLibrary(dll);
    std::printf("[harness] PASS\n");
    return 0;
}
