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
#include <cwchar>
#include <exception>
#include <functional>

#include "hsmp_native.h"
#include "ue4ss_reflect.h"
#include "ue4ss_reflect_names.h"
#include "ue4ss_find_route.hpp"
#include "caller_frame_walk.hpp"
#include "ue4ss_pins.h"

namespace RC::Unreal { class UObject; struct FFrame; }

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
    hsmp_reflect::FindRoute g_find_route;

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
        return g_find_route.find(reinterpret_cast<const wchar_t*>(path));
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

    using CallerCallback = std::function<void(RC::Unreal::UObject*, RC::Unreal::FFrame&, void*)>;
    using RegisterCaller = void (*)(CallerCallback);
    // Verified against the deployed DLL with timestamp 6ABB88FA / image 13BA000.
    // The legacy overload avoids a replica of private FCallbackOptions layout.
    constexpr const char* kCallerExports[] = {
        "?Node@FFrame@Unreal@RC@@QEAAAEAPEAVUFunction@23@XZ",
        "?Object@FFrame@Unreal@RC@@QEAAAEAPEAVUObject@23@XZ",
        "?PreviousFrame@FFrame@Unreal@RC@@QEAAAEAPEAU123@XZ",
        "?RegisterProcessInternalPreCallback@Hook@Unreal@RC@@YAXV?$function@$$A6AXPEAVUObject@Unreal@RC@@AEAUFFrame@23@PEAX@Z@std@@@Z",
        "?RegisterProcessInternalPostCallback@Hook@Unreal@RC@@YAXV?$function@$$A6AXPEAVUObject@Unreal@RC@@AEAUFFrame@23@PEAX@Z@std@@@Z",
        "?RegisterProcessLocalScriptFunctionPreCallback@Hook@Unreal@RC@@YAXV?$function@$$A6AXPEAVUObject@Unreal@RC@@AEAUFFrame@23@PEAX@Z@std@@@Z",
        "?RegisterProcessLocalScriptFunctionPostCallback@Hook@Unreal@RC@@YAXV?$function@$$A6AXPEAVUObject@Unreal@RC@@AEAUFFrame@23@PEAX@Z@std@@@Z",
    };
    struct CallerRole
    {
        const wchar_t* path;
        uint64_t weak{}, address{}, name{};
        ULONGLONG looked{}, window{};
        unsigned emitted{}, limit;
        ULONGLONG named_at{};
    };
    CallerRole g_caller_roles[] = {
        {L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage",0,0,0,0,0,0,64},
        {L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage",0,0,0,0,0,0,32},
        {L"/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C:Collision Hit",0,0,0,0,0,0,32},
        {L"/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:ReceiveBeginPlay",0,0,0,0,0,0,16},
        {L"/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:ExecuteUbergraph_Constraint_Weapon_Stuck_BP",0,0,0,0,0,0,4},
        {L"/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:ReceiveTick",0,0,0,0,0,0,4},
        {L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Initiate",0,0,0,0,0,0,16},
        {L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Delayed",0,0,0,0,0,0,16},
        {L"/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C:ExecuteUbergraph_ModularWeaponBP",0,0,0,0,0,0,4},
    };
    static_assert(sizeof(g_caller_roles) / sizeof(g_caller_roles[0]) <= 32);
    hsmp_caller::Getters g_caller_getters{};
    HsmpCallerSink g_caller_sink{};
    bool g_caller_submitted = false;
    uint64_t g_caller_seq{};
    thread_local bool g_caller_reading = false;

    struct CallerClasses { void* classes[3]{}; unsigned known{}; };
    CallerClasses caller_classes()
    {
        CallerClasses out{};
        void* core = g_api.static_find(nullptr, nullptr, L"/Script/CoreUObject.Class", false);
        const auto core_weak = r_weak(core);
        if (!core_weak || r_resolve(core_weak) != core) return out;
        const wchar_t* paths[] = {
            L"/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C",
            L"/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C",
            L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C",
        };
        for (unsigned i = 0; i < 3; ++i)
        {
            void* cls = g_api.static_find(nullptr, nullptr, paths[i], false);
            const auto weak = r_weak(cls);
            if (weak && r_resolve(weak) == cls && g_api.is_a(cls, core))
            {
                out.classes[i] = cls;
                out.known |= 1u << i;
            }
        }
        return out; // These live pointers are local to this exact callback only.
    }
    HsmpCallerIdentity caller_identity(void* obj, const CallerClasses* classes = nullptr)
    {
        HsmpCallerIdentity out{};
        out.address = reinterpret_cast<uint64_t>(obj);
        if (!obj) return out;
        // The pointer comes from the currently executing engine callback/frame.
        // Confirm its actual GUObjectArray slot before reading any other fields.
        out.weak = r_weak(obj);
        if (!out.weak || r_resolve(out.weak) != obj) return out;
        out.available = 1;
        out.persistent = (out.weak >> 32) != 0 ? 1u : 0u;
        out.name = pack(*g_api.name_private(obj));
        void* cls = *g_api.class_private(obj);
        out.class_address = reinterpret_cast<uint64_t>(cls);
        out.class_weak = r_weak(cls);
        if (!out.class_weak || r_resolve(out.class_weak) != cls) out.class_weak = 0;
        else { out.class_available = 1; out.class_name = pack(*g_api.name_private(cls)); }
        if (classes)
        {
            out.actor_kind_known = classes->known;
            for (unsigned i = 0; i < 3; ++i)
                if (classes->classes[i] && g_api.is_a(obj, classes->classes[i])) out.actor_kind |= 1u << i;
        }
        return out;
    }
    uint32_t caller_candidates(void* node, ULONGLONG now)
    {
        if (!node) return 0;
        // Node belongs to the live engine frame. Reject unrelated names before
        // GUObjectArray/class reads or StaticFindObject on this very hot hook.
        const uint64_t name = pack(*g_api.name_private(node));
        uint32_t candidates = 0;
        for (unsigned i = 0; i < sizeof(g_caller_roles) / sizeof(g_caller_roles[0]); ++i)
        {
            auto& r = g_caller_roles[i];
            if (!r.name && (!r.named_at || now - r.named_at >= 1000))
            {
                r.named_at = now;
                r.name = make_fname(std::wcsrchr(r.path, L':') + 1, 0); // FNAME_Find only; no engine-name additions.
            }
            if (r.name && r.name == name)
            {
                if (hsmp_caller::defer_different_node(reinterpret_cast<uint64_t>(node), r.address, r.looked, now)) continue;
                candidates |= 1u << i;
            }
        }
        return candidates;
    }
    bool caller_verify(void* node, ULONGLONG now, unsigned candidate)
    {
        auto& r = g_caller_roles[candidate - 1];
        const auto id = caller_identity(node);
        if (!id.available) return 0;
        const bool nonpersistent = r.weak && !(r.weak >> 32);
        // A zero serial cannot prove cross-callback identity. Resolve the
        // complete reflected path afresh for every eligible observation.
        void* actual = r.weak && !nonpersistent ? r_resolve(r.weak) : nullptr;
        if (!actual || reinterpret_cast<uint64_t>(actual) != r.address)
        {
            if (!nonpersistent && r.looked && now - r.looked < 1000) return 0;
            r.looked = now;
            actual = g_api.static_find(nullptr, nullptr, r.path, false);
            const auto found = caller_identity(actual);
            r.weak = found.available ? found.weak : 0;
            r.address = found.available ? found.address : 0;
            r.name = found.available ? found.name : 0;
            if (!found.available) actual = nullptr;
        }
        return actual == node;
    }
    unsigned caller_role(void* node, ULONGLONG now)
    {
        const uint32_t candidates = caller_candidates(node, now);
        for (unsigned i = 0; i < sizeof(g_caller_roles) / sizeof(g_caller_roles[0]); ++i)
            if ((candidates & (1u << i)) && caller_verify(node, now, i + 1)) return i + 1;
        return 0;
    }
    void caller_observe(unsigned hook, unsigned phase, RC::Unreal::UObject* context, RC::Unreal::FFrame& frame)
    {
        // Do not establish a game thread from a diagnostic callback. The Rust
        // guard requires an earlier real Native::frame, and never waits on locks.
        if (!g_caller_sink || g_caller_reading || !hsmp_native_caller_thread_ok()) return;
        g_caller_reading = true;
        struct ReadingGuard { ~ReadingGuard() { g_caller_reading = false; } } guard;
        try
        {
            void** slot = g_caller_getters.node(&frame);
            if (!slot || !*slot) return;
            const ULONGLONG now = GetTickCount64();
            const uint32_t candidates = caller_candidates(*slot, now);
            const unsigned role = hsmp_caller::sample_candidates(candidates,
                sizeof(g_caller_roles) / sizeof(g_caller_roles[0]), g_caller_roles, now,
                [&](unsigned candidate) { return caller_verify(*slot, now, candidate); });
            if (!role) return;
            HsmpCallerEvent event{};
            event.seq = ++g_caller_seq;
            event.hook = hook; event.phase = phase; event.role = role; event.observed = 1;
            const auto classes = caller_classes();
            event.context = caller_identity(context, &classes);
            const auto result = hsmp_caller::walk(&frame, g_caller_getters, [&](std::size_t i, void* n, void* o) {
                event.frames[i].node = caller_identity(n);
                event.frames[i].object = caller_identity(o, &classes);
                event.frames[i].role = caller_role(n, now);
            });
            event.count = static_cast<uint32_t>(result.count);
            event.end = static_cast<uint32_t>(result.end);
            // Emits inside this exact callback, independent of Lua POST order.
            // There is no cached origin API and no authority derived from history.
            g_caller_sink(event);
        }
        catch (const std::exception&)
        {
            HsmpCallerEvent event{}; event.seq = ++g_caller_seq; event.hook = hook; event.phase = phase; event.end = 5;
            g_caller_sink(event); // C++ unwind is unavailable; no SEH/AV swallowing.
        }
        catch (...)
        {
            HsmpCallerEvent event{}; event.seq = ++g_caller_seq; event.hook = hook; event.phase = phase; event.end = 6;
            g_caller_sink(event);
        }
    }
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
    g_find_route.slow=g_api.static_find;
    // The optional path API never honors the offline unpinned override. Resolve
    // actual exports, not an inferred Shipping or UE4SS function address.
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(ue);
    const auto* pe=dos->e_magic==IMAGE_DOS_SIGNATURE&&dos->e_lfanew>=0&&dos->e_lfanew<=65536?
        reinterpret_cast<const IMAGE_NT_HEADERS64*>(reinterpret_cast<const uint8_t*>(ue)+dos->e_lfanew):nullptr;
    g_find_route.path=nullptr;g_find_route.available=nullptr;
    if(pe&&pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.TimeDateStamp==HSMP_UE4SS_TIMESTAMP&&pe->OptionalHeader.SizeOfImage==HSMP_UE4SS_SIZE_OF_IMAGE){
        g_find_route.path=reinterpret_cast<hsmp_reflect::PathFind>(GetProcAddress(ue,hsmp_reflect::kFindNames[0]));
        g_find_route.available=reinterpret_cast<hsmp_reflect::HashAvailable>(GetProcAddress(ue,hsmp_reflect::kFindNames[1]));
    }
    hsmp_native_set_reflect(&g_vt);
    return nullptr;
}

void hsmp_reflect_set_find_log(HsmpFindRouteLog sink){g_find_route.logger.store(sink);}

const HsmpReflect* hsmp_reflect_table()
{
    return g_api.internal_index ? &g_vt : nullptr;
}

const char* hsmp_reflect_caller_register(HsmpCallerSink sink)
{
    if (g_caller_submitted) return "caller callbacks already submitted";
    if (!sink || !g_api.internal_index || !g_api.index_to_item || !g_api.item_object || !g_api.item_serial
        || !g_api.item_valid || !g_api.name_private || !g_api.class_private || !g_api.static_find || !g_api.fname_ctor || !g_api.is_a)
        return "reflection unavailable";
    HMODULE ue = GetModuleHandleW(L"UE4SS.dll");
    if (!ue) return "UE4SS.dll not loaded";
    // This probe never honors HSMPNATIVE_ALLOW_UNPINNED. Only the real pinned
    // host can justify the reference-getter and legacy std::function ABI.
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(ue);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return "caller host PE unavailable";
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(reinterpret_cast<const uint8_t*>(ue) + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->FileHeader.TimeDateStamp != HSMP_UE4SS_TIMESTAMP
        || nt->OptionalHeader.SizeOfImage != HSMP_UE4SS_SIZE_OF_IMAGE)
        return "caller host is not the actual pinned UE4SS build";
    void* got[7]{};
    for (unsigned i = 0; i < 7; ++i)
    {
        got[i] = reinterpret_cast<void*>(GetProcAddress(ue, kCallerExports[i]));
        if (!got[i]) return kCallerExports[i];
    }
    g_caller_getters = {reinterpret_cast<hsmp_caller::Getter>(got[0]), reinterpret_cast<hsmp_caller::Getter>(got[1]),
                       reinterpret_cast<hsmp_caller::Getter>(got[2])};
    g_caller_sink = sink;
    // Reserve before submission. Even an uncertain partial registration cannot
    // cause a second observer allocation or an untracked callback replacement.
    g_caller_submitted = true;
    try
    {
        for (unsigned i = 3; i < 7; ++i)
        {
            const unsigned hook = i < 5 ? 1u : 2u;
            const unsigned phase = (i == 3 || i == 5) ? 1u : 2u;
            reinterpret_cast<RegisterCaller>(got[i])([hook, phase](RC::Unreal::UObject* c, RC::Unreal::FFrame& f, void*) {
                caller_observe(hook, phase, c, f);
            });
        }
        return nullptr;
    }
    catch (...) { return "caller callback submission uncertain"; }
}
