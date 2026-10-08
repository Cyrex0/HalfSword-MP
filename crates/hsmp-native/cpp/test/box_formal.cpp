// Exercises the production formal reader with the shipped function's child counts.
#include <cstdio>
#include <cstdlib>
#include <string_view>
#include <vector>
#include "box_formal.hpp"
namespace
{
using namespace hsmp_box;
unsigned checks{};
void check(bool ok,const char* message)
{ ++checks; if (!ok) { std::fprintf(stderr,"FAIL %s\n",message); std::exit(1); } }
struct Field { HsmpProp p{}; std::uint64_t flags{}; Field* next{}; };
struct Fixture
{
    std::vector<Field> fields;
    unsigned calls{}, described{}, traversed{};
    std::int32_t second_count_delta{}, second_storage_delta{};
    bool cycle_on_first{}, shorten_on_first{};
    explicit Fixture(unsigned n):fields(n)
    {
        for (unsigned i=0;i<n;++i)
        {
            fields[i].p.name=2000+i; fields[i].p.cls=9;
            fields[i].p.offset=256+static_cast<std::int32_t>(i)*8; fields[i].p.size=8;
            fields[i].next=i+1<n ? &fields[i+1] : nullptr;
        }
        fields[0].p={11,7,0,128,8,0,0,{}}; fields[0].flags=kFormalParm;
        fields[14].p={12,7,0,160,8,0,0,{}}; fields[14].flags=kFormalParm|0x80000; // InstancedReference
    }
    std::int32_t props(void*,HsmpProp* out,std::int32_t cap,std::int32_t* storage)
    {
        ++calls; *storage=32768+(calls==2 ? second_storage_delta : 0);
        std::int32_t n=0;
        for (auto* f=&fields[0];f;f=f->next)
        {
            if (n<cap) { out[n]=f->p; ++described; }
            if (++n>kMaxFunctionProperties) break;
        }
        return n+(calls==2 ? second_count_delta : 0);
    }
    void* first(void*)
    {
        if (cycle_on_first) fields.back().next=&fields[0];
        if (shorten_on_first) fields[fields.size()-2].next=nullptr;
        return &fields[0];
    }
    void* next(void* p) { ++traversed; return static_cast<Field*>(p)->next; }
    bool flags(void* p,std::uint64_t mask) { return (static_cast<Field*>(p)->flags&mask)!=0; }
};
bool read(Fixture& f,HsmpProp& p,FormalDiagnostic& d,std::uint64_t name=12,bool returns=false,unsigned size=200)
{ return read_formal(f,nullptr,name,7,size,returns,p,d); }
}
int main()
{
    using namespace hsmp_box;
    HsmpProp p{}; FormalDiagnostic d{};
    {
        Fixture f(1070);
        check(read(f,p,d) && p.offset==160 && p.size==8,"shipped Get Damage Hit Box among 1070 children");
        check(d.count==1070 && d.copied==1070 && d.visited==1070 && f.described==1070,
            "complete enumeration includes locals beyond the old 512 cap");
        check(read(f,p,d,11) && p.offset==128,"shipped Get Damage Damaged Mesh offset");
    }
    {
        Fixture f(343); f.fields[0].p.offset=0;
        check(read(f,p,d,11,false,225) && p.offset==0,"shipped Deal Complex Damage Hit Component");
        check(read(f,p,d,12,false,225) && p.offset==160,"shipped Deal Complex Damage Hit Box");
    }
    {
        Fixture f(4096); check(read(f,p,d) && d.visited==4096,"complete reader accepts its hard ceiling");
        Fixture over(4097); check(!read(over,p,d) && d.failure==std::string_view("children_limit")
            && over.calls==1 && over.described==0,"overflow refused before heap descriptor fill");
        Fixture cycle(1070); cycle.fields.back().next=&cycle.fields[0];
        check(!read(cycle,p,d) && d.count==4097 && cycle.calls==1,"cyclic count scan refused at reflector ceiling");
    }
    {
        Fixture f(1070); f.second_count_delta=-1;
        check(!read(f,p,d) && d.failure==std::string_view("children_changed") && f.traversed==0,
            "changed count refused before field flag access");
        Fixture g(1070); g.second_storage_delta=8;
        check(!read(g,p,d) && d.failure==std::string_view("storage_changed"),"changed property storage refused");
        Fixture h(1070); h.cycle_on_first=true;
        check(!read(h,p,d) && d.failure==std::string_view("chain_long_or_cycle") && h.traversed==1070,
            "cycle introduced after copying cannot qualify a truncated prefix");
        Fixture j(1070); j.shorten_on_first=true;
        check(!read(j,p,d) && d.failure==std::string_view("chain_short"),"shortened chain refused");
    }
    {
        Fixture f(1070); f.fields[1069].p=f.fields[14].p; f.fields[1069].flags=kFormalParm;
        check(!read(f,p,d) && d.failure==std::string_view("duplicate_name") && p.name==0,
            "duplicate at tail beyond old cap invalidates earlier valid input");
        Fixture missing(1070); missing.fields[14].p.name=99;
        check(!read(missing,p,d) && d.failure==std::string_view("name_missing"),"missing name refused");
        Fixture local(1070); local.fields[14].flags=0;
        check(!read(local,p,d) && d.failure==std::string_view("not_parameter"),"same-name local refused");
        Fixture type(1070); type.fields[14].p.cls=9;
        check(!read(type,p,d) && d.failure==std::string_view("property_type"),"wrong property class refused");
        for (const auto flags : {kFormalOut,kFormalReturn,kFormalReference})
        { Fixture indirect(1070); indirect.fields[14].flags|=flags;
          check(!read(indirect,p,d) && d.failure==std::string_view("indirect_parameter"),"indirect input refused"); }
        Fixture small(1070); small.fields[14].p.size=4;
        check(!read(small,p,d) && d.failure==std::string_view("pointer_size"),"non-pointer element size refused");
        Fixture negative(1070); negative.fields[14].p.offset=-1;
        check(!read(negative,p,d) && d.failure==std::string_view("offset_bounds"),"negative offset refused");
        Fixture beyond(1070); beyond.fields[14].p.offset=196;
        check(!read(beyond,p,d) && d.failure==std::string_view("offset_bounds"),"pointer over parameter boundary refused");
        Fixture tail(1070); tail.fields[14].p.offset=1024;
        check(!read(tail,p,d) && d.failure==std::string_view("offset_bounds"),"local storage is not formal parameter storage");
    }
    {
        Fixture f(16); f.fields[14].flags=kFormalParm|kFormalOut|kFormalReturn;
        check(read(f,p,d,12,true),"by-value ReturnValue may carry OutParm");
        f.fields[14].flags=kFormalParm;
        check(!read(f,p,d,12,true) && d.failure==std::string_view("return_flag"),"ReturnValue requires actual return flag");
        f.fields[14].flags=kFormalParm|kFormalReturn|kFormalReference;
        check(!read(f,p,d,12,true),"reference ReturnValue refused");
        check(!read(f,p,d,0) && d.failure==std::string_view("name_unavailable"),"unavailable requested FName refused");
        check(!read(f,p,d,12,false,0) && d.failure==std::string_view("parameter_size"),"zero parameter storage refused");
        check(!read(f,p,d,12,false,65536),"oversized parameter storage refused");
        check(!read(f,p,d,12,false,40000) && d.failure==std::string_view("storage_bounds"),"parameter storage beyond property storage refused");
    }
    std::printf("box_formal: %u checks PASS\n",checks);
}
