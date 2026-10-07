// The engine-reflection vtable shared by cpp/src/ue4ss_reflect.cpp (the provider, option F)
// and the Rust staticlib (crates/hsmp-native/src/reflect.rs, the consumer). Keep both in sync.
#pragma once

#include <cstdint>

#define HSMP_REFLECT_ABI 1u

extern "C" {

struct HsmpProp
{
    uint64_t name;   // FName (ComparisonIndex | Number << 32)
    uint64_t cls;    // the property class's FName
    uint64_t sub;    // StructProperty: the struct's FName
    int32_t offset;
    int32_t size;    // ElementSize
    uint8_t bool_offset;
    uint8_t bool_mask;
    uint8_t pad[6];
};
static_assert(sizeof(HsmpProp) == 40, "HsmpProp layout (reflect.rs)");

struct HsmpReflect
{
    uint32_t abi;
    uint32_t pad;
    uint64_t (*fname)(const uint16_t* s, int32_t add);
    void* (*find)(const uint16_t* path);
    int32_t (*is_a)(void* obj, void* cls);
    void* (*class_of)(void* obj);
    int32_t (*props)(void* ustruct, HsmpProp* out, int32_t cap, int32_t* size);
    int32_t (*obj_prop)(void* obj, const uint16_t* name, HsmpProp* out);
    void (*call)(void* obj, void* func, void* params);
    uint64_t (*weak)(void* obj);
    void* (*resolve)(uint64_t weak);
};

// Rust (reflect.rs): install the table (null removes it).
void hsmp_native_set_reflect(const HsmpReflect* vt);

} // extern "C"

// ue4ss_reflect.cpp: resolve the UE4SS exports and register; nullptr = registered, else the
// first missing export's name.
const char* hsmp_reflect_register();

// Opt-in observational journal. Every pointer is borrowed only inside a live
// script-hook callback; the sink receives scalar observations synchronously.
struct HsmpCallerIdentity
{
    uint64_t address{}, weak{}, class_address{}, class_weak{}, name{}, class_name{};
    uint32_t available{}, persistent{}, class_available{}, actor_kind{}, actor_kind_known{};
};
struct HsmpCallerFrame { HsmpCallerIdentity node{}, object{}; uint32_t role{}; };
struct HsmpCallerEvent
{
    uint64_t seq{};
    uint32_t hook{}, phase{}, role{}, count{}, end{}, observed{};
    HsmpCallerIdentity context{};
    HsmpCallerFrame frames[16]{};
};
using HsmpCallerSink = void (*)(const HsmpCallerEvent&);
// Legacy registration is a submitted callback, not returned-ID proof. Only
// actual observed callbacks qualify journal coverage. Never creates a factory.
const char* hsmp_reflect_caller_register(HsmpCallerSink sink);
