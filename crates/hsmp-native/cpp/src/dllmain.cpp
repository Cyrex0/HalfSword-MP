// HSMPNative: the UE4SS C++ mod (option F) that gives every HSMP* Lua mod the global table
// `HSMPNative` (HSMP-SHM, docs/development/ipc-shared-memory.md).
//
// All IPC logic lives in the Rust staticlib crates/hsmp-native (hsmp_native.lib); this file
// only does UE4SS registration:
//   * CppUserModBase replica (abi/ue4ss_abi.hpp) pinned to UE4SS e3ba1016;
//   * on_lua_start -> hsmp_native_open(L, mod_name) for every Lua mod whose name starts with
//     "HSMP", before that mod's main.lua runs;
//   * refuses to register on an un-pinned UE4SS.dll (PE TimeDateStamp + SizeOfImage); the Lua
//     facade then falls back to option E (hsmp_lua.dll).
//     HSMPNATIVE_ALLOW_UNPINNED=1 overrides that for the offline harness only;
//   * pins its own module (the process-global IPC state must outlive any unload).
//
// No threads, no sockets, no SEH swallowing; UObject access only through ue4ss_reflect.cpp (native sampling / servo). Native code never raises Lua
// errors (no luaL_error / luaL_check*).

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include <cstdarg>
#include <cstdio>
#include <mutex>
#include <string>

#include "lua.hpp"

#include "gen/hsmp_ipc.h"
#include "hsmp_native.h"
#include "ue4ss_reflect.h"
#include "native_presentation.h"
#include "box_snapshot_probe.h"
#include "ue4ss_abi.hpp"
#include "ue4ss_pins.h"

// The generated header must agree with the Rust crate the DLL links.
static_assert(HSMP_IPC_ABI_MAJOR == 2, "hsmp_ipc.h ABI major");

#define HSMPNATIVE_VERSION "1.0.0"

using namespace RC;

namespace
{
    std::mutex g_log_mutex;
    std::wstring g_log_path;
    HMODULE g_self = nullptr;

    void init_log_path(HMODULE self)
    {
        wchar_t buf[MAX_PATH]{};
        GetModuleFileNameW(self, buf, MAX_PATH);
        std::wstring p = buf; // ...\Mods\HSMPNative\dlls\main.dll
        for (int i = 0; i < 2; ++i)
        {
            auto cut = p.find_last_of(L"\\/");
            if (cut != std::wstring::npos) p.resize(cut);
        }
        g_log_path = p + L"\\HSMPNative.log";
    }

    void logf(const char* fmt, ...)
    {
        char msg[1024];
        va_list ap;
        va_start(ap, fmt);
        vsnprintf(msg, sizeof msg, fmt, ap);
        va_end(ap);
        SYSTEMTIME t;
        GetLocalTime(&t);
        char line[1200];
        snprintf(line, sizeof line, "[%02d:%02d:%02d.%03d][pid %lu][tid %lu] %s\n", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds,
                 GetCurrentProcessId(), GetCurrentThreadId(), msg);
        OutputDebugStringA(line);
        std::lock_guard lk(g_log_mutex);
        if (g_log_path.empty()) return;
        FILE* f = nullptr;
        if (_wfopen_s(&f, g_log_path.c_str(), L"ab") == 0 && f)
        {
            fputs(line, f);
            fclose(f);
        }
    }

    // Is the loaded UE4SS.dll the exact build abi/ue4ss_pins.h was generated from?
    bool ue4ss_pinned(const char** why)
    {
        HMODULE host = GetModuleHandleW(L"UE4SS.dll");
        if (!host)
        {
            *why = "UE4SS.dll not loaded";
            return false;
        }
        auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(host);
        auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(reinterpret_cast<const BYTE*>(host) + dos->e_lfanew);
        if (nt->FileHeader.TimeDateStamp != HSMP_UE4SS_TIMESTAMP || nt->OptionalHeader.SizeOfImage != HSMP_UE4SS_SIZE_OF_IMAGE)
        {
            *why = "UE4SS.dll is not the pinned build " HSMP_UE4SS_GIT_SHA;
            return false;
        }
        *why = "pinned " HSMP_UE4SS_GIT_SHA;
        return true;
    }

    bool allow_unpinned()
    {
        char v[8]{};
        return GetEnvironmentVariableA("HSMPNATIVE_ALLOW_UNPINNED", v, sizeof v) > 0 && v[0] == '1';
    }

    bool wants_api(StringViewType mod_name)
    {
        return mod_name.size() >= 4 && mod_name.substr(0, 4) == L"HSMP";
    }

    std::string narrow(StringViewType w)
    {
        std::string s;
        s.reserve(w.size());
        for (wchar_t c : w) s.push_back(c < 128 ? static_cast<char>(c) : '?');
        return s;
    }

    void pin_self()
    {
        HMODULE h = nullptr;
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                           reinterpret_cast<LPCWSTR>(&pin_self), &h);
    }

    void caller_journal(const HsmpCallerEvent& e)
    {
        logf("NATIVE_CALLER seq=%llu observed=%u hook=%u phase=%u role=%u count=%u end=%u chain_complete=%u context=%llu weak=%llu class=%llu class_weak=%llu class_name=%llu class_available=%u actor_kind=%u actor_kind_known=%u context_available=%u persistent=%u lua_post_order=unproved authority=false",
             static_cast<unsigned long long>(e.seq), e.observed, e.hook, e.phase, e.role, e.count, e.end,
             e.observed && e.end == 0 ? 1u : 0u, static_cast<unsigned long long>(e.context.address),
             static_cast<unsigned long long>(e.context.weak), static_cast<unsigned long long>(e.context.class_address),
             static_cast<unsigned long long>(e.context.class_weak), static_cast<unsigned long long>(e.context.class_name),
             e.context.class_available, e.context.actor_kind, e.context.actor_kind_known, e.context.available, e.context.persistent);
        for (uint32_t i = 0; i < e.count; ++i)
        {
            const auto& f = e.frames[i];
            logf("NATIVE_CALLER_FRAME seq=%llu depth=%u role=%u node=%llu node_weak=%llu node_name=%llu node_available=%u object=%llu object_weak=%llu object_name=%llu object_available=%u class=%llu class_weak=%llu class_name=%llu class_available=%u actor_kind=%u actor_kind_known=%u persistent=%u",
                 static_cast<unsigned long long>(e.seq), i, f.role, static_cast<unsigned long long>(f.node.address),
                 static_cast<unsigned long long>(f.node.weak), static_cast<unsigned long long>(f.node.name), f.node.available,
                 static_cast<unsigned long long>(f.object.address), static_cast<unsigned long long>(f.object.weak),
                 static_cast<unsigned long long>(f.object.name), f.object.available,
                 static_cast<unsigned long long>(f.object.class_address), static_cast<unsigned long long>(f.object.class_weak),
                 static_cast<unsigned long long>(f.object.class_name), f.object.class_available, f.object.actor_kind, f.object.actor_kind_known, f.object.persistent);
        }
    }
} // namespace

class HSMPNativeMod final : public CppUserModBase
{
    bool m_enabled = false;
    std::string m_why;

  public:
    HSMPNativeMod()
    {
        ModName = L"HSMPNative";
        ModVersion = L"" HSMPNATIVE_VERSION;
        ModDescription = L"HSMP shared-memory IPC (HSMP-SHM) for the HSMP Lua mods";
        ModAuthors = L"HSMP";
        const char* why = "";
        bool pinned = ue4ss_pinned(&why);
        m_enabled = pinned || allow_unpinned();
        m_why = why;
        logf("HSMPNative %s constructed (ue4ss abi %s, lua %s): %s -> %s", HSMPNATIVE_VERSION, HSMP_UE4SS_GIT_SHA, LUA_RELEASE, why,
             m_enabled ? (pinned ? "enabled" : "enabled (HSMPNATIVE_ALLOW_UNPINNED)") : "REFUSED: not registering (facade falls back)");
        // Engine reflection for native sampling / servo, on the pinned build only (an unpinned
        // UE4SS.dll may move any of these exports; the facade then stays on the Lua path).
        if (m_enabled)
        {
            const char* missing = hsmp_reflect_register();
            if(!missing) hsmp_presentation_register(hsmp_reflect_table());
            logf("native sampling: %s%s", missing ? "unavailable, missing UE4SS export " : "engine reflection registered", missing ? missing : "");
        }
    }

    ~HSMPNativeMod() override
    {
        logf("HSMPNative destroyed");
    }

    auto on_unreal_init() -> void override
    {
        logf("on_unreal_init");
        char caller_probe[8]{};
        if (!m_enabled || GetEnvironmentVariableA("HSMP_NATIVE_CALLER_PROBE", caller_probe, sizeof caller_probe) != 1
            || caller_probe[0] != '1') return;
        const char* unavailable = hsmp_reflect_caller_register(caller_journal);
        logf("native caller journal: %s detail=%s ids=unavailable coverage=unavailable factory=false limit=16 default=off",
             unavailable ? "unavailable" : "callbacks submitted", unavailable ? unavailable : "await actual callbacks");
    }

    auto on_lua_start(StringViewType mod_name, LuaMadeSimple::Lua& lua, LuaMadeSimple::Lua&, LuaMadeSimple::Lua&, LuaMadeSimple::Lua*)
            -> void override
    {
        if (!m_enabled || !wants_api(mod_name)) return;
        lua_State* L = lua.get_lua_state();
        if (!L) return;
        int top = lua_gettop(L);
        std::string name = narrow(mod_name);
        if (hsmp_native_open(L, name.c_str()) != 1)
        {
            logf("hsmp_native_open failed for '%s'", name.c_str());
            lua_settop(L, top);
            return;
        }
        // Diagnostics fields only C++ knows.
        if (lua_getglobal(L, "HSMPNative") == LUA_TTABLE)
        {
            lua_pushstring(L, hsmp_lua_lock_mode());
            lua_setfield(L, -2, "lock_mode");
            lua_pushstring(L, m_why.c_str());
            lua_setfield(L, -2, "ue4ss");
        }
        lua_settop(L, top);
        hsmp_box_probe_install(L); // Default OFF; explicit bounded developer enrollment only.
        logf("registered HSMPNative into Lua mod '%s' (L=%p, lock=%s)", name.c_str(), (void*)L, hsmp_lua_lock_mode());
    }
};

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID)
{
    if (reason == DLL_PROCESS_ATTACH)
    {
        g_self = inst;
        DisableThreadLibraryCalls(inst);
        init_log_path(inst);
    }
    return TRUE;
}

extern "C" __declspec(dllexport) CppUserModBase* start_mod()
{
    pin_self();
    logf("start_mod");
    return new HSMPNativeMod();
}

extern "C" __declspec(dllexport) void uninstall_mod(CppUserModBase* mod)
{
    logf("uninstall_mod");
    delete mod;
}
