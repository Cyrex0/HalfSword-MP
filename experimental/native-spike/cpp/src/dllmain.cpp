// HSMPNative transport spike: a UE4SS C++ mod (option F) for UE4SS v3.0.1 git e3ba1016.
//
// On load it:
//   1. injects a global table `HSMPNative` into every Lua mod whose name starts
//      with "HSMP" (via CppUserModBase::on_lua_start, which UE4SS fires BEFORE
//      the mod's main.lua runs);
//   2. offers HSMPNative.process_event_add(a, b) / frame_count(), which call
//      UFunctions through UObject::ProcessEvent;
//   3. links the Rust staticlib hsmp_core_stub (UDP socket on a background
//      thread + lock-free SPSC queues) and exposes net_* functions that only
//      touch the queues, so they never block the game thread.
//
// Native code never raises Lua errors (no luaL_error / luaL_check*): a longjmp
// through C++ frames skips destructors. Failures return `nil, "message"`.

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include <cstdarg>
#include <cstdio>
#include <mutex>
#include <string>

#include "lua.hpp"

#include "hsmp_core.h"
#include "ue4ss_abi.hpp"
#include "ue4ss_pins.h"

#define HSMPNATIVE_VERSION "0.0.1-m1"

using namespace RC;
using RC::Unreal::UFunction;
using RC::Unreal::UObject;

namespace
{
    // ---------------------------------------------------------------- logging
    std::mutex g_log_mutex;
    std::wstring g_log_path;

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
        snprintf(line, sizeof line, "[%02d:%02d:%02d.%03d][tid %lu] %s\n", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds,
                 GetCurrentThreadId(), msg);
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

    // Prints through the Lua state's own `print` so the line lands in UE4SS.log.
    void lua_print(lua_State* L, const char* text)
    {
        lua_getglobal(L, "print");
        if (lua_type(L, -1) != LUA_TFUNCTION)
        {
            lua_pop(L, 1);
            return;
        }
        lua_pushstring(L, text);
        if (lua_pcall(L, 1, 0, 0) != LUA_OK) lua_pop(L, 1);
    }

    // ------------------------------------------------------------ lua helpers
    int nil_err(lua_State* L, const char* msg)
    {
        lua_pushnil(L);
        lua_pushstring(L, msg);
        return 2;
    }

    bool opt_int(lua_State* L, int idx, lua_Integer def, lua_Integer& out)
    {
        if (lua_isnoneornil(L, idx))
        {
            out = def;
            return true;
        }
        int ok = 0;
        out = lua_tointegerx(L, idx, &ok);
        return ok != 0;
    }

    const char* opt_str(lua_State* L, int idx, const char* def)
    {
        if (lua_isnoneornil(L, idx)) return def;
        return lua_type(L, idx) == LUA_TSTRING ? lua_tostring(L, idx) : nullptr;
    }

    const char* core_err(int32_t e)
    {
        switch (e)
        {
        case HSMP_E_NOT_RUNNING: return "not running";
        case HSMP_E_ALREADY_RUNNING: return "already running";
        case HSMP_E_BAD_ARG: return "bad argument";
        case HSMP_E_IO: return "socket error";
        case HSMP_E_FULL: return "queue full";
        case HSMP_E_TOO_BIG: return "message too big";
        case HSMP_E_BUSY: return "busy (concurrent caller)";
        case HSMP_E_EMPTY: return "empty";
        case HSMP_E_TOO_SMALL: return "buffer too small";
        default: return "internal error";
        }
    }

    // --------------------------------------------------------- ProcessEvent
    UObject* find_object(const wchar_t* path)
    {
        return Unreal::UObjectGlobals::StaticFindObject_InternalSlow(nullptr, nullptr, path, false);
    }

    // No C++ objects in this frame, so SEH is allowed. A fault inside
    // ProcessEvent becomes an error string instead of a game crash (spike
    // diagnostics only; production code must not rely on this).
    DWORD guarded_process_event(UObject* obj, UFunction* fn, void* params)
    {
        __try
        {
            obj->ProcessEvent(fn, params);
            return 0;
        }
        __except (EXCEPTION_EXECUTE_HANDLER)
        {
            return GetExceptionCode();
        }
    }

    // HSMPNative.process_event_add(a, b) -> a + b computed by
    // /Script/Engine.KismetMathLibrary:Add_IntInt (params A@0, B@4, ReturnValue@8;
    // offsets from UE4SS_ObjectDump.txt).
    int l_process_event_add(lua_State* L)
    {
        lua_Integer a = 0, b = 0;
        if (!opt_int(L, 1, 2, a) || !opt_int(L, 2, 40, b)) return nil_err(L, "process_event_add(a:int, b:int)");
        UObject* cdo = find_object(L"/Script/Engine.Default__KismetMathLibrary");
        auto* fn = reinterpret_cast<UFunction*>(find_object(L"/Script/Engine.KismetMathLibrary:Add_IntInt"));
        if (!cdo || !fn) return nil_err(L, "StaticFindObject failed for KismetMathLibrary / Add_IntInt");
        struct
        {
            int32_t A, B, ReturnValue;
        } params{static_cast<int32_t>(a), static_cast<int32_t>(b), INT32_MIN};
        if (DWORD code = guarded_process_event(cdo, fn, &params))
        {
            char m[96];
            snprintf(m, sizeof m, "ProcessEvent raised SEH exception 0x%08lX", code);
            logf("%s", m);
            return nil_err(L, m);
        }
        lua_pushinteger(L, params.ReturnValue);
        return 1;
    }

    // HSMPNative.frame_count() -> GFrameCounter via KismetSystemLibrary:GetFrameCount (ReturnValue int64 @0).
    int l_frame_count(lua_State* L)
    {
        UObject* cdo = find_object(L"/Script/Engine.Default__KismetSystemLibrary");
        auto* fn = reinterpret_cast<UFunction*>(find_object(L"/Script/Engine.KismetSystemLibrary:GetFrameCount"));
        if (!cdo || !fn) return nil_err(L, "StaticFindObject failed for KismetSystemLibrary / GetFrameCount");
        struct
        {
            int64_t ReturnValue;
        } params{-1};
        if (DWORD code = guarded_process_event(cdo, fn, &params))
        {
            char m[96];
            snprintf(m, sizeof m, "ProcessEvent raised SEH exception 0x%08lX", code);
            return nil_err(L, m);
        }
        lua_pushinteger(L, params.ReturnValue);
        return 1;
    }

    // ---------------------------------------------------------- misc / net
    int l_ping(lua_State* L)
    {
        char s[512];
        snprintf(s, sizeof s, "pong from HSMPNative %s (option F) | ue4ss %s | %s | %s | lock=%s | tid=%lu", HSMPNATIVE_VERSION,
                 HSMP_UE4SS_GIT_SHA, LUA_RELEASE, hsmp_core_version(), hsmp_lua_lock_mode(), GetCurrentThreadId());
        lua_pushstring(L, s);
        return 1;
    }

    int l_thread_id(lua_State* L)
    {
        lua_pushinteger(L, static_cast<lua_Integer>(GetCurrentThreadId()));
        return 1;
    }

    int l_now_us(lua_State* L)
    {
        lua_pushinteger(L, static_cast<lua_Integer>(hsmp_core_now_us()));
        return 1;
    }

    int l_lock_mode(lua_State* L)
    {
        lua_pushstring(L, hsmp_lua_lock_mode());
        return 1;
    }

    int l_net_start(lua_State* L)
    {
        const char* bind = opt_str(L, 1, "127.0.0.1:0");
        const char* peer = opt_str(L, 2, "self");
        if (!bind || !peer) return nil_err(L, "net_start(bind:string?, peer:string?)");
        int32_t r = hsmp_core_start(bind, peer);
        if (r <= 0) return nil_err(L, core_err(r));
        logf("net_start bind=%s peer=%s -> port %d", bind, peer, r);
        lua_pushinteger(L, r);
        return 1;
    }

    int l_net_stop(lua_State* L)
    {
        hsmp_core_stop();
        lua_pushboolean(L, 1);
        return 1;
    }

    int l_net_send(lua_State* L)
    {
        if (lua_type(L, 1) != LUA_TSTRING) return nil_err(L, "net_send(data:string)");
        size_t n = 0;
        const char* p = lua_tolstring(L, 1, &n);
        int32_t r = hsmp_core_send(reinterpret_cast<const uint8_t*>(p), n);
        if (r != HSMP_OK) return nil_err(L, core_err(r));
        lua_pushboolean(L, 1);
        return 1;
    }

    // net_poll(max = 64) -> array of strings; never blocks.
    int l_net_poll(lua_State* L)
    {
        lua_Integer max = 64;
        if (!opt_int(L, 1, 64, max)) return nil_err(L, "net_poll(max:int?)");
        if (max < 0) max = 0;
        if (max > 4096) max = 4096;
        lua_createtable(L, 0, 0);
        uint8_t buf[2048];
        for (lua_Integer i = 1; i <= max; ++i)
        {
            int32_t n = hsmp_core_poll(buf, sizeof buf);
            if (n < 0) break;
            lua_pushlstring(L, reinterpret_cast<const char*>(buf), static_cast<size_t>(n));
            lua_rawseti(L, -2, i);
        }
        return 1;
    }

    int l_net_stats(lua_State* L)
    {
        HsmpStats s{};
        hsmp_core_stats(&s);
        lua_createtable(L, 0, 11);
        auto set = [L](const char* k, lua_Integer v) {
            lua_pushinteger(L, v);
            lua_setfield(L, -2, k);
        };
        set("sent", (lua_Integer)s.sent);
        set("recv", (lua_Integer)s.recv);
        set("send_err", (lua_Integer)s.send_err);
        set("recv_err", (lua_Integer)s.recv_err);
        set("drop_in_full", (lua_Integer)s.drop_in_full);
        set("drop_out_full", (lua_Integer)s.drop_out_full);
        set("loops", (lua_Integer)s.loops);
        set("inq_len", s.inq_len);
        set("outq_len", s.outq_len);
        set("running", s.running);
        set("local_port", s.local_port);
        return 1;
    }

    void register_api(lua_State* L)
    {
        static const luaL_Reg funcs[] = {
                {"ping", l_ping},
                {"thread_id", l_thread_id},
                {"now_us", l_now_us},
                {"lock_mode", l_lock_mode},
                {"process_event_add", l_process_event_add},
                {"frame_count", l_frame_count},
                {"net_start", l_net_start},
                {"net_stop", l_net_stop},
                {"net_send", l_net_send},
                {"net_poll", l_net_poll},
                {"net_stats", l_net_stats},
        };
        lua_createtable(L, 0, static_cast<int>(std::size(funcs)) + 3);
        for (const auto& f : funcs)
        {
            lua_pushcclosure(L, f.func, 0);
            lua_setfield(L, -2, f.name);
        }
        lua_pushstring(L, HSMPNATIVE_VERSION);
        lua_setfield(L, -2, "version");
        lua_pushstring(L, HSMP_UE4SS_GIT_SHA);
        lua_setfield(L, -2, "ue4ss_abi");
        lua_pushinteger(L, hsmp_core_max_payload());
        lua_setfield(L, -2, "max_payload");
        lua_setglobal(L, "HSMPNative");
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
} // namespace

class HSMPNativeMod final : public CppUserModBase
{
  public:
    HSMPNativeMod()
    {
        ModName = L"HSMPNative";
        ModVersion = L"0.0.1-m1";
        ModDescription = L"HSMP in-process native transport (M1 spike)";
        ModAuthors = L"HSMP";
        logf("HSMPNative %s constructed (ue4ss abi %s, lua %s, %s)", HSMPNATIVE_VERSION, HSMP_UE4SS_GIT_SHA, LUA_RELEASE,
             hsmp_core_version());
    }

    ~HSMPNativeMod() override
    {
        hsmp_core_stop();
        logf("HSMPNative destroyed");
    }

    auto on_unreal_init() -> void override
    {
        logf("on_unreal_init");
    }

    auto on_lua_start(StringViewType mod_name, LuaMadeSimple::Lua& lua, LuaMadeSimple::Lua&, LuaMadeSimple::Lua&, LuaMadeSimple::Lua*)
            -> void override
    {
        if (!wants_api(mod_name)) return;
        lua_State* L = lua.get_lua_state();
        if (!L) return;
        int top = lua_gettop(L);
        register_api(L);
        lua_settop(L, top);
        std::string name = narrow(mod_name);
        logf("registered HSMPNative into Lua mod '%s' (L=%p, lock=%s)", name.c_str(), (void*)L, hsmp_lua_lock_mode());
        char line[256];
        snprintf(line, sizeof line, "[HSMPNative] M1 OK: HSMPNative table registered into '%s' (lock=%s)\n", name.c_str(),
                 hsmp_lua_lock_mode());
        lua_print(L, line);
        lua_settop(L, top);
    }
};

static HMODULE g_self = nullptr;

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
    logf("start_mod");
    return new HSMPNativeMod();
}

extern "C" __declspec(dllexport) void uninstall_mod(CppUserModBase* mod)
{
    logf("uninstall_mod");
    delete mod;
}
