// Mock UE4SS.dll for the offline harness. Exports exactly the symbols listed in
// abi/UE4SS.def (the harness fails to load main.dll if one is missing), and
// fakes two UFunctions so ProcessEvent plumbing can be checked without the game.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <cstdint>
#include <cstring>
#include <cwchar>

#include "ue4ss_abi.hpp"

namespace RC
{
    CppUserModBase::CppUserModBase() = default;
    CppUserModBase::~CppUserModBase() = default;

    // Real class: first member is `lua_State* m_lua_state`.
    auto LuaMadeSimple::Lua::get_lua_state() const -> lua_State*
    {
        return *reinterpret_cast<lua_State* const*>(this);
    }
} // namespace RC

namespace
{
    struct FakeObject
    {
        const wchar_t* path;
    };
    FakeObject g_math_cdo{L"/Script/Engine.Default__KismetMathLibrary"};
    FakeObject g_sys_cdo{L"/Script/Engine.Default__KismetSystemLibrary"};
    FakeObject g_add{L"/Script/Engine.KismetMathLibrary:Add_IntInt"};
    FakeObject g_frames{L"/Script/Engine.KismetSystemLibrary:GetFrameCount"};
    FakeObject* g_all[] = {&g_math_cdo, &g_sys_cdo, &g_add, &g_frames};
} // namespace

namespace RC::Unreal
{
    namespace UObjectGlobals
    {
        auto StaticFindObject_InternalSlow(UClass*, UObject*, const wchar_t* name, bool) -> UObject*
        {
            for (FakeObject* o : g_all)
            {
                if (name && wcscmp(name, o->path) == 0) return reinterpret_cast<UObject*>(o);
            }
            return nullptr;
        }
    } // namespace UObjectGlobals

    auto UObject::ProcessEvent(UFunction* function, void* params) -> void
    {
        auto* fn = reinterpret_cast<FakeObject*>(function);
        auto* self = reinterpret_cast<FakeObject*>(this);
        if (fn == &g_add && self == &g_math_cdo)
        {
            auto* p = static_cast<int32_t*>(params);
            p[2] = p[0] + p[1];
        }
        else if (fn == &g_frames && self == &g_sys_cdo)
        {
            static int64_t frame = 123456; // advances like GFrameCounter
            *static_cast<int64_t*>(params) = frame++;
        }
        else
        {
            RaiseException(0xE0000001, 0, 0, nullptr); // exercises the SEH guard
        }
    }
} // namespace RC::Unreal
