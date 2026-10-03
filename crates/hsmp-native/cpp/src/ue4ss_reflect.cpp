// HSMPNative: the engine-reflection vtable for native sampling / servo.
//
// Resolves a handful of RC::Unreal exports of UE4SS.dll with GetProcAddress (NOT import-lib
// imports: a missing export must disable native sampling, never stop main.dll from loading)
// and hands the Rust side a C table (crates/hsmp-native/src/reflect.rs). Registered only on
// the pinned UE4SS build (or HSMPNATIVE_ALLOW_UNPINNED=1 for the offline harness).
//
// Calling convention (x64 MSVC): an instance method is called like a free function with
// `this` first; a class-type return value goes through a hidden pointer right after `this`;
// a reference return is a pointer. The layouts we rely on were read off UE4SS.dll e3ba1016:
//   FName              8 bytes {ComparisonIndex u32, Number u32}  (FName(uint,uint) ctor)
//   weak handle        8 bytes {ObjectIndex i32, SerialNumber i32}  (built from the object
//                      array exports, never FWeakObjectPtr: see r_weak)
//   FFieldClassVariant 16 bytes {pointer, bool IsUObject}            (FField::GetClass)
// The export names are checked against the real DLL by test/reflect_check.cpp.
//
// Nothing here caches an engine object; no SEH; every call runs on the game thread inside a
// Lua -> native call (the Rust side guarantees it).
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include <cstdint>
#include <cstring>

#include "hsmp_native.h"
#include "ue4ss_reflect.h"
#include "ue4ss_reflect_names.h"

namespace
{
    struct FName8
    {
        uint32_t index;
        uint32_t number;
    };
    static_assert(sizeof(FName8) == 8);

    struct Variant16
    {
        void* ptr;
        bool is_uobject;
        uint8_t pad[7];
    };
    static_assert(sizeof(Variant16) == 16);

    using PFNameCtor = void* (*)(void* self, const wchar_t* s, int find, void* fn_override);
    using PStaticFind = void* (*)(void* cls, void* outer, const wchar_t* name, bool exact);
    using PProcessEvent = void (*)(void* self, void* fn, void* params);
    using PIsA = bool (*)(const void* self, void* cls);
    using PClassPrivate = void* const* (*)(const void* self);
    using PChildProps = void** (*)(void* self);
    using PIntRef = int* (*)(void* self);
    using PNextProp = void* (*)(void* self);
    using PFieldFName = FName8* (*)(const void* self, FName8* ret);
    using PFieldClass = Variant16* (*)(void* self, Variant16* ret);
    using PVariantFName = FName8* (*)(const Variant16* self, FName8* ret);
    using PObjPtrRef = void** (*)(void* self);
    using PByteRef = uint8_t* (*)(void* self);
    using PPropByName = void* (*)(void* self, const wchar_t* name);
    using PNamePrivate = const FName8* (*)(const void* self);
    using PIndexOf = const int* (*)(const void* self);
    using PIndexToItem = void* (*)(int index);
    using PItemObject = void** (*)(void* item);
    using PItemSerial = int* (*)(void* item);
    using PItemValid = bool (*)(const void* item, bool even_if_pending_kill);

    struct Api
    {
        PFNameCtor fname_ctor;
        PStaticFind static_find;
        PProcessEvent process_event;
        PIsA is_a;
        PClassPrivate class_private;
        PChildProps child_props;
        PIntRef props_size;
        PNextProp next_prop;
        PFieldFName field_fname;
        PFieldClass field_class;
        PVariantFName variant_fname;
        PIntRef offset_internal;
        PIntRef element_size;
        PObjPtrRef struct_of;
        PByteRef bool_byte_offset;
        PByteRef bool_byte_mask;
        PPropByName prop_by_name;
        PNamePrivate name_private;
        PIndexOf internal_index;
        PIndexToItem index_to_item;
        PItemObject item_object;
        PItemSerial item_serial;
        PItemValid item_valid;
    };
    Api g_api{};

    using hsmp_reflect::kNames;
    static_assert(sizeof(kNames) / sizeof(kNames[0]) == sizeof(Api) / sizeof(void*), "kNames <-> Api");

    uint64_t pack(FName8 n)
    {
        return static_cast<uint64_t>(n.index) | (static_cast<uint64_t>(n.number) << 32);
    }

    uint64_t make_fname(const wchar_t* s, int add)
    {
        FName8 n{};
        g_api.fname_ctor(&n, s, add ? 1 : 0, nullptr);
        return pack(n);
    }

    uint64_t g_struct_property = 0;
    uint64_t g_bool_property = 0;

    void describe(void* prop, HsmpProp* out)
    {
        std::memset(out, 0, sizeof *out);
        FName8 n{};
        g_api.field_fname(prop, &n);
        out->name = pack(n);
        Variant16 v{};
        g_api.field_class(prop, &v);
        FName8 c{};
        g_api.variant_fname(&v, &c);
        out->cls = pack(c);
        out->offset = *g_api.offset_internal(prop);
        out->size = *g_api.element_size(prop);
        if (g_struct_property == 0) g_struct_property = make_fname(L"StructProperty", 1);
        if (g_bool_property == 0) g_bool_property = make_fname(L"BoolProperty", 1);
        if (out->cls == g_struct_property)
        {
            void* s = *g_api.struct_of(prop);
            if (s) out->sub = pack(*g_api.name_private(s));
        }
        else if (out->cls == g_bool_property)
        {
            out->bool_offset = *g_api.bool_byte_offset(prop);
            out->bool_mask = *g_api.bool_byte_mask(prop);
        }
    }

    // ---- the vtable functions ----
    uint64_t r_fname(const uint16_t* s, int32_t add)
    {
        return make_fname(reinterpret_cast<const wchar_t*>(s), add);
    }
    void* r_find(const uint16_t* path)
    {
        return g_api.static_find(nullptr, nullptr, reinterpret_cast<const wchar_t*>(path), false);
    }
    int32_t r_is_a(void* obj, void* cls)
    {
        return g_api.is_a(obj, cls) ? 1 : 0;
    }
    void* r_class_of(void* obj)
    {
        return *g_api.class_private(obj);
    }
    int32_t r_props(void* ustruct, HsmpProp* out, int32_t cap, int32_t* size)
    {
        *size = *g_api.props_size(ustruct);
        int32_t n = 0;
        for (void* f = *g_api.child_props(ustruct); f; f = g_api.next_prop(f))
        {
            if (n < cap) describe(f, &out[n]);
            ++n;
            if (n > 4096) break; // a corrupt chain never loops forever
        }
        return n;
    }
    int32_t r_obj_prop(void* obj, const uint16_t* name, HsmpProp* out)
    {
        void* p = g_api.prop_by_name(obj, reinterpret_cast<const wchar_t*>(name));
        if (!p) return 0;
        describe(p, out);
        return 1;
    }
    void r_call(void* obj, void* func, void* params)
    {
        g_api.process_event(obj, func, params);
    }
    // Weak handles without FWeakObjectPtr: UE4SS's FWeakObjectPtr(obj) allocates a serial
    // number for an object that has none yet (FUObjectArray::AllocateSerialNumber), and that
    // faults (uncatchable AV) on this build for every freshly spawned pawn / mesh / weapon.
    // Nothing here writes to the engine: the handle is {index, the serial as it is now},
    // packed like FWeakObjectPtr. Serial 0 (most objects in-game, /Script
    // UFunctions included) skips the serial check: the caller compares the result with the
    // object it expects (within a call; kept handles are pinned to their address, reflect.rs).
    uint64_t r_weak(void* obj)
    {
        if (!obj) return 0;
        const int index = *g_api.internal_index(obj);
        void* item = g_api.index_to_item(index);
        if (!item || *g_api.item_object(item) != obj) return 0;
        const int serial = *g_api.item_serial(item);
        return static_cast<uint64_t>(static_cast<uint32_t>(index)) | (static_cast<uint64_t>(static_cast<uint32_t>(serial)) << 32);
    }
    void* r_resolve(uint64_t w)
    {
        if (w == 0) return nullptr;
        const int index = static_cast<int>(static_cast<uint32_t>(w));
        const int serial = static_cast<int>(static_cast<uint32_t>(w >> 32));
        void* item = g_api.index_to_item(index);
        if (!item || (serial != 0 && *g_api.item_serial(item) != serial) || !g_api.item_valid(item, false)) return nullptr;
        return *g_api.item_object(item);
    }

    HsmpReflect g_vt{HSMP_REFLECT_ABI, 0, r_fname, r_find, r_is_a, r_class_of, r_props, r_obj_prop, r_call, r_weak, r_resolve};
} // namespace

// Resolve every export; register the vtable when all exist. Returns the first missing name
// (nullptr = registered).
const char* hsmp_reflect_register()
{
    HMODULE ue = GetModuleHandleW(L"UE4SS.dll");
    if (!ue) return "UE4SS.dll not loaded";
    void* got[sizeof(kNames) / sizeof(kNames[0])]{};
    for (size_t i = 0; i < sizeof(kNames) / sizeof(kNames[0]); ++i)
    {
        got[i] = reinterpret_cast<void*>(GetProcAddress(ue, kNames[i]));
        if (!got[i]) return kNames[i];
    }
    std::memcpy(&g_api, got, sizeof g_api);
    hsmp_native_set_reflect(&g_vt);
    return nullptr;
}
