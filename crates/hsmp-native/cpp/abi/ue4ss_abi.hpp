// Minimal, header-only view of the UE4SS C++ mod ABI for the exact build
// UE4SS v3.0.1 experimental, git e3ba1016 (Half Sword's deployed UE4SS.dll).
//
// Why a replica instead of RE-UE4SS's own headers: the real
// Mod/CppUserModBase.hpp transitively includes GUI/LiveView and the Unreal
// (UEPseudo) headers, and UEPseudo is a private Epic-gated submodule that is
// not available on this machine. Everything below was checked against the
// shipped binary, not just the source:
//
//  * layout: ??0CppUserModBase disassembly writes vptr@0, vector@0x08,
//    five std::wstring @0x20/0x40/0x60/0x80/0xA0 -> sizeof == 0xC0;
//  * vtable: CppMod::fire_update -> [vptr+0x08], fire_unreal_init -> +0x10,
//    fire_ui_init -> +0x18, fire_program_start -> +0x20,
//    LuaMod::fire_on_lua_start_for_cpp_mods -> +0x28 (new, no name),
//    +0x30 (new, with name), +0x38 / +0x40 (deprecated vector overloads).
//    MSVC groups overloads and emits them in reverse declaration order, which
//    is why the declaration order below must stay verbatim;
//  * every symbol we import is listed in abi/UE4SS.def, generated from the
//    DLL's export table by tools/gen_ue4ss_pins.py.
//
// The offline harness (test/harness.cpp) dispatches into the built mod using
// the raw offsets above, so a drift in this header fails the build's tests.
#pragma once

#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

#pragma warning(push)
#pragma warning(disable : 4100) // unnamed-by-design default virtual bodies

#ifndef RC_UE4SS_API
#define RC_UE4SS_API __declspec(dllimport)
#endif

struct lua_State;

namespace RC
{
    using CharType = wchar_t;
    using StringType = std::basic_string<CharType>;
    using StringViewType = std::basic_string_view<CharType>;

    namespace GUI
    {
        class GUITab;
    }

    namespace LuaMadeSimple
    {
        class Lua
        {
          public:
            RC_UE4SS_API auto get_lua_state() const -> lua_State*;
        };
    } // namespace LuaMadeSimple

    namespace Unreal
    {
        class UObject;
        class UClass;
        class UFunction;
        class UScriptStruct;
        class FProperty;
        class FField;

        // Native sampling / servo (cpp/src/ue4ss_reflect.cpp resolves these with GetProcAddress, never through
        // the import lib; the declarations exist so the offline mock exports the same mangled
        // names, and test/reflect_check.cpp compares them with the real UE4SS.dll).
        enum EFindName
        {
            FNAME_Find,
            FNAME_Add,
        };

        class FName
        {
          public:
            uint32_t ComparisonIndex;
            uint32_t Number;
            RC_UE4SS_API FName(const wchar_t* name, EFindName find_type, void* function_address_override);
        };
        static_assert(sizeof(FName) == 8, "FName is 8 bytes in UE4SS e3ba1016");

        class FFieldClassVariant
        {
          public:
            void* Container;
            bool bIsUObject;
            RC_UE4SS_API auto GetFName() const -> FName;
        };
        static_assert(sizeof(FFieldClassVariant) == 16, "FFieldClassVariant is 16 bytes in UE4SS e3ba1016");

        template <typename T>
        class TObjectPtr
        {
          public:
            T* Pointer;
        };

        class UObjectBase
        {
          public:
            RC_UE4SS_API auto IsA(UClass* cls) const -> bool;
            RC_UE4SS_API auto GetClassPrivate() const -> const UClass*&;
            RC_UE4SS_API auto GetNamePrivate() const -> const FName&;
            RC_UE4SS_API auto GetInternalIndex_Private() const -> const int&;
        };

        class UStruct
        {
          public:
            RC_UE4SS_API auto GetChildProperties() -> FField*&;
            RC_UE4SS_API auto GetPropertiesSize() -> int&;
        };

        class FField
        {
          public:
            RC_UE4SS_API auto GetNextFieldAsProperty() -> FProperty*;
            RC_UE4SS_API auto GetFName() const -> FName;
            RC_UE4SS_API auto GetClass() -> FFieldClassVariant;
        };

        class FProperty
        {
          public:
            RC_UE4SS_API auto GetOffset_Internal() -> int&;
            RC_UE4SS_API auto GetElementSize() -> int&;
        };

        class FStructProperty
        {
          public:
            RC_UE4SS_API auto GetStruct() -> TObjectPtr<UScriptStruct>&;
        };

        class FBoolProperty
        {
          public:
            RC_UE4SS_API auto GetByteOffset() -> uint8_t&;
            RC_UE4SS_API auto GetByteMask() -> uint8_t&;
        };

        // Weak handles are built from these, NOT from FWeakObjectPtr: UE4SS's
        // FWeakObjectPtr(obj) calls FUObjectArray::AllocateSerialNumber for an object that has
        // no serial yet (every freshly spawned pawn / mesh / weapon), and that path faults with
        // an uncatchable access violation on this build (seen on both game instances).
#ifdef GetObject
#undef GetObject // wingdi.h's GetObjectW macro would rename the export
#endif
        struct FUObjectItem
        {
          private:
            RC_UE4SS_API auto GetObject() -> UObjectBase*&;

          public:
            RC_UE4SS_API auto GetSerialNumber() -> int&;
            RC_UE4SS_API auto IsValid(bool even_if_pending_kill) const -> bool;
        };

        class FUObjectArray
        {
          public:
            RC_UE4SS_API static auto IndexToObject(int index) -> FUObjectItem*;
        };

        class UObject
        {
          public:
            RC_UE4SS_API auto ProcessEvent(UFunction* function, void* params) -> void;
            RC_UE4SS_API auto GetPropertyByNameInChain(const wchar_t* name) -> FProperty*;
        };

        namespace UObjectGlobals
        {
            RC_UE4SS_API auto StaticFindObject_InternalSlow(UClass* object_class, UObject* in_outer, const wchar_t* name, bool exact_class) -> UObject*;
        }
    } // namespace Unreal

    // Verbatim member/virtual order of UE4SS/include/Mod/CppUserModBase.hpp @ e3ba1016.
    class CppUserModBase
    {
      protected:
        std::vector<std::shared_ptr<GUI::GUITab>> GUITabs{};

      public:
        StringType ModName{};
        StringType ModVersion{};
        StringType ModDescription{};
        StringType ModAuthors{};
        StringType ModIntendedSDKVersion{};

      public:
        RC_UE4SS_API CppUserModBase();
        RC_UE4SS_API virtual ~CppUserModBase();

      public:
        RC_UE4SS_API virtual auto on_update() -> void {}
        RC_UE4SS_API virtual auto on_unreal_init() -> void {}
        RC_UE4SS_API virtual auto on_ui_init() -> void {}
        RC_UE4SS_API virtual auto on_program_start() -> void {}
        RC_UE4SS_API virtual auto on_lua_start(StringViewType mod_name,
                                               LuaMadeSimple::Lua& lua,
                                               LuaMadeSimple::Lua& main_lua,
                                               LuaMadeSimple::Lua& async_lua,
                                               std::vector<LuaMadeSimple::Lua*>& hook_luas) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_start(LuaMadeSimple::Lua& lua,
                                               LuaMadeSimple::Lua& main_lua,
                                               LuaMadeSimple::Lua& async_lua,
                                               std::vector<LuaMadeSimple::Lua*>& hook_luas) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_stop(StringViewType mod_name,
                                              LuaMadeSimple::Lua& lua,
                                              LuaMadeSimple::Lua& main_lua,
                                              LuaMadeSimple::Lua& async_lua,
                                              std::vector<LuaMadeSimple::Lua*>& hook_luas) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_stop(LuaMadeSimple::Lua& lua,
                                              LuaMadeSimple::Lua& main_lua,
                                              LuaMadeSimple::Lua& async_lua,
                                              std::vector<LuaMadeSimple::Lua*>& hook_luas) -> void
        {
        }
        RC_UE4SS_API virtual auto on_dll_load(StringViewType dll_name) -> void {}
        RC_UE4SS_API virtual auto render_tab() -> void {}
        RC_UE4SS_API virtual auto on_lua_start(StringViewType mod_name,
                                               LuaMadeSimple::Lua& lua,
                                               LuaMadeSimple::Lua& main_lua,
                                               LuaMadeSimple::Lua& async_lua,
                                               LuaMadeSimple::Lua* hook_lua) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_start(LuaMadeSimple::Lua& lua,
                                               LuaMadeSimple::Lua& main_lua,
                                               LuaMadeSimple::Lua& async_lua,
                                               LuaMadeSimple::Lua* hook_lua) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_stop(StringViewType mod_name,
                                              LuaMadeSimple::Lua& lua,
                                              LuaMadeSimple::Lua& main_lua,
                                              LuaMadeSimple::Lua& async_lua,
                                              LuaMadeSimple::Lua* hook_lua) -> void
        {
        }
        RC_UE4SS_API virtual auto on_lua_stop(LuaMadeSimple::Lua& lua,
                                              LuaMadeSimple::Lua& main_lua,
                                              LuaMadeSimple::Lua& async_lua,
                                              LuaMadeSimple::Lua* hook_lua) -> void
        {
        }
        RC_UE4SS_API virtual auto on_cpp_mods_loaded() -> void {}
    };

    static_assert(sizeof(StringType) == 0x20, "MSVC std::wstring layout");
} // namespace RC

// Offsets proven from the shipped UE4SS.dll (see header comment).
namespace hsmp_abi
{
    constexpr std::size_t kSizeofCppUserModBase = 0xC0;
    constexpr std::size_t kVtOnUpdate = 0x08;
    constexpr std::size_t kVtOnUnrealInit = 0x10;
    constexpr std::size_t kVtOnUiInit = 0x18;
    constexpr std::size_t kVtOnProgramStart = 0x20;
    constexpr std::size_t kVtOnLuaStartNoName = 0x28;
    constexpr std::size_t kVtOnLuaStartNamed = 0x30;
    constexpr std::size_t kVtOnLuaStartNoNameDeprecated = 0x38;
    constexpr std::size_t kVtOnLuaStartNamedDeprecated = 0x40;
} // namespace hsmp_abi

#pragma warning(pop)

static_assert(sizeof(RC::CppUserModBase) == hsmp_abi::kSizeofCppUserModBase, "CppUserModBase layout drifted from UE4SS e3ba1016");
