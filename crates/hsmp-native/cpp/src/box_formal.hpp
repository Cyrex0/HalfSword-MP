// Complete, bounded function-property inspection. No engine objects are retained.
#pragma once
#include <vector>
#include "box_snapshot_pair.hpp"
#include "ue4ss_reflect.h"

namespace hsmp_box
{
inline constexpr std::int32_t kMaxFunctionProperties = 4096;
inline constexpr std::uint64_t kFormalParm=0x80, kFormalOut=0x100,
    kFormalReturn=0x400, kFormalReference=0x08000000;
struct FormalDiagnostic
{
    const char* failure{"none"};
    std::int32_t count{-1}, copied{-1}, storage{-1}, copied_storage{-1}, offset{-1}, size{-1};
    unsigned parms_size{}, visited{}, matches{};
};
// Ops delegates to the pinned reflection provider. props() counts all children,
// including Blueprint locals, and itself has the same 4096-entry ceiling.
template<class Ops>
bool read_formal(Ops& ops,void* function,std::uint64_t wanted,std::uint64_t object_property,
                 unsigned parms_size,bool returns,HsmpProp& out,FormalDiagnostic& d)
{
    out={}; d={}; d.parms_size=parms_size;
    const auto fail=[&](const char* why) { d.failure=why; return false; };
    if (!wanted || !object_property) return fail("name_unavailable");
    if (!parms_size || parms_size>65535) return fail("parameter_size");
    d.count=ops.props(function,nullptr,0,&d.storage);
    if (d.count<=0) return fail("children_unavailable");
    if (d.count>kMaxFunctionProperties) return fail("children_limit");
    if (d.storage<static_cast<std::int32_t>(parms_size)) return fail("storage_bounds");
    std::vector<HsmpProp> props(static_cast<std::size_t>(d.count));
    d.copied=ops.props(function,props.data(),d.count,&d.copied_storage);
    if (d.copied!=d.count) return fail("children_changed");
    if (d.copied_storage!=d.storage) return fail("storage_changed");
    HsmpProp candidate{};
    for (void* field=ops.first(function);field;field=ops.next(field))
    {
        // A cycle or growth cannot be truncated into successful qualification.
        if (d.visited>=static_cast<unsigned>(d.count)) return fail("chain_long_or_cycle");
        const auto& p=props[d.visited++];
        if (p.name!=wanted) continue;
        d.offset=p.offset; d.size=p.size;
        if (++d.matches!=1) return fail("duplicate_name");
        const bool parm=ops.flags(field,kFormalParm);
        const bool indirect=ops.flags(field,kFormalReference)
            || (!returns && ops.flags(field,kFormalOut|kFormalReturn));
        if (!input_bounds(p.offset,p.size,parms_size,p.cls==object_property,parm,indirect))
        {
            if (p.cls!=object_property) return fail("property_type");
            if (!parm) return fail("not_parameter");
            if (indirect) return fail("indirect_parameter");
            if (p.size!=sizeof(void*)) return fail("pointer_size");
            return fail("offset_bounds");
        }
        if (returns && !ops.flags(field,kFormalReturn)) return fail("return_flag");
        candidate=p;
    }
    if (d.visited!=static_cast<unsigned>(d.count)) return fail("chain_short");
    if (d.matches!=1) return fail("name_missing");
    out=candidate; return true;
}
}
