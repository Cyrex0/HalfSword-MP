/*
 * Replacement for RE-UE4SS deps/first/LuaRaw/src/luauser.c in HSMPNative's
 * private copy of Lua 5.4.7.
 *
 * UE4SS's luaconf.h routes lua_lock/lua_unlock to LuaLock/LuaUnlock, which
 * enter ONE process-wide CRITICAL_SECTION (`static struct Gl` in luauser.c,
 * lazily (re)initialised by LuaLock, deleted by LuaLockFinal whenever any
 * state closes). Our private Lua copy would otherwise get its own, unrelated
 * critical section, so API calls made from our C functions would not be
 * serialised against UE4SS's own Lua threads.
 *
 * When the loaded UE4SS.dll is the exact build we pinned (PE TimeDateStamp +
 * SizeOfImage, abi/ue4ss_pins.h), our LuaLock/LuaUnlock jump to UE4SS's own
 * LuaLock/LuaUnlock (RVAs from UE4SS.pdb) -> identical semantics, including
 * its lazy re-initialisation ("host" mode). Otherwise we use a private
 * critical section ("private" mode, = what an official UE4SS C++ mod that
 * links LuaRaw statically gets) and report it via hsmp_lua_lock_mode().
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include "lua.h"
#include "lstate.h"
#include "lobject.h"
#include "lfunc.h"
#include "ue4ss_pins.h"

/*
 * Build-time ABI check of the dual-copy setup: every Lua internal struct our
 * private copy will read or write on UE4SS-owned states must have exactly the
 * size it has inside UE4SS.dll (sizes extracted from UE4SS.pdb by
 * tools/ue4ss_pins). A luaconf.h / Lua version drift fails the build here.
 */
/*
 * Error-handling ABI: UE4SS compiled its Lua as C (setjmp/longjmp), so must we. A C++-compiled
 * copy would throw C++ exceptions through UE4SS-owned frames.
 */
#if defined(__cplusplus) || defined(LUA_COMPILED_AS_CPP)
#error "HSMPNative private Lua copy must be compiled as C (LUA_COMPILED_AS_CPP is not supported)"
#endif
#if !HSMP_UE4SS_LUA_COMPILED_AS_C
#error "pinned UE4SS.dll was not verified to have a C-compiled Lua"
#endif

#define HSMP_SAME_SIZE(T) _Static_assert(sizeof(T) == HSMP_UE4SS_SIZEOF_##T, "Lua ABI drift vs UE4SS.dll: sizeof(" #T ")")
HSMP_SAME_SIZE(lua_State);
HSMP_SAME_SIZE(global_State);
HSMP_SAME_SIZE(CallInfo);
HSMP_SAME_SIZE(Table);
HSMP_SAME_SIZE(TString);
HSMP_SAME_SIZE(Udata);
HSMP_SAME_SIZE(CClosure);
HSMP_SAME_SIZE(LClosure);
HSMP_SAME_SIZE(Proto);
HSMP_SAME_SIZE(UpVal);
HSMP_SAME_SIZE(lua_Debug);
HSMP_SAME_SIZE(stringtable);

typedef void (*lock_fn)(lua_State*);

static CRITICAL_SECTION g_private_cs;
static lock_fn g_host_lock = NULL;
static lock_fn g_host_unlock = NULL;
static const char* g_mode = "unresolved";
static INIT_ONCE g_once = INIT_ONCE_STATIC_INIT;

static BOOL CALLBACK resolve_lock(PINIT_ONCE once, PVOID param, PVOID* ctx)
{
    (void)once;
    (void)param;
    (void)ctx;
    InitializeCriticalSection(&g_private_cs);
    g_mode = "private (UE4SS.dll not loaded)";

    HMODULE host = GetModuleHandleW(L"UE4SS.dll");
    if (!host)
    {
        return TRUE;
    }
    const IMAGE_DOS_HEADER* dos = (const IMAGE_DOS_HEADER*)host;
    const IMAGE_NT_HEADERS64* nt = (const IMAGE_NT_HEADERS64*)((const BYTE*)host + dos->e_lfanew);
    if (nt->FileHeader.TimeDateStamp != HSMP_UE4SS_TIMESTAMP || nt->OptionalHeader.SizeOfImage != HSMP_UE4SS_SIZE_OF_IMAGE)
    {
        g_mode = "private (UE4SS.dll is not the pinned build " HSMP_UE4SS_GIT_SHA ")";
        return TRUE;
    }
    g_host_lock = (lock_fn)((BYTE*)host + HSMP_UE4SS_RVA_LuaLock);
    g_host_unlock = (lock_fn)((BYTE*)host + HSMP_UE4SS_RVA_LuaUnlock);
    g_mode = "host (UE4SS LuaLock/LuaUnlock, pinned " HSMP_UE4SS_GIT_SHA ")";
    return TRUE;
}

static void resolve(void)
{
    InitOnceExecuteOnce(&g_once, resolve_lock, NULL, NULL);
}

/* We never create or close states (UE4SS owns them), so these do nothing. */
void LuaLockInitial(lua_State* L)
{
    (void)L;
    resolve();
}

void LuaLockFinal(lua_State* L)
{
    (void)L;
}

void LuaLock(lua_State* L)
{
    resolve();
    if (g_host_lock)
        g_host_lock(L);
    else
        EnterCriticalSection(&g_private_cs);
}

void LuaUnlock(lua_State* L)
{
    resolve();
    if (g_host_unlock)
        g_host_unlock(L);
    else
        LeaveCriticalSection(&g_private_cs);
}

const char* hsmp_lua_lock_mode(void)
{
    resolve();
    return g_mode;
}
