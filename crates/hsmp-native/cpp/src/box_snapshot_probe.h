// Optional proof-only native Box observer. Never writes engine state.
#pragma once
#include "box_snapshot_pair.hpp"
#include "box_enrollment_timing.hpp"
struct lua_State;
namespace hsmp_box
{
struct ObjectInput { const wchar_t* path{}; std::uint64_t address{}; };
struct Enrollment
{
    ObjectInput world{}, pawn{}, mesh{}, box{}, box_owner{};
    std::uint64_t match_id{}; std::uint32_t round{}, life{};
};
using Sink = void (*)(unsigned phase, void* context, void* frame);
struct Provider
{
    bool (*on_thread)();
    std::uint64_t (*now_ms)();
    bool (*enroll)(const Enrollment&,Scope&,Reason&);
    bool (*submit)(Sink,Reason&);
    // Returns no role for a non-target function, without a global role/name lookup.
    bool (*key)(void* context,void* frame,Key&,Reason&);
    bool (*snapshot)(const Key&,void* frame,Snapshot&,Reason&);
    // Optional bounded scalar detail for a failed enrollment, never runtime authority.
    const char* (*enrollment_detail)(){};
    const EnrollmentTiming* (*enrollment_timing)(){};
};
}
// ue4ss_reflect_box.cpp provides the pinned host; tests supply a bounded fake.
const hsmp_box::Provider& hsmp_reflect_box_provider();
void hsmp_box_probe_install(lua_State* L);

