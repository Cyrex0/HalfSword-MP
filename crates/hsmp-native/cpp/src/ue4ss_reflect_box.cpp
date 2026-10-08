// Default-OFF Box snapshot provider. Reads only enrolled objects in the pinned host.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <cstring>
#include <cstdio>
#include <functional>
#include "box_formal.hpp"
#include "box_snapshot_probe.h"
#include "box_snapshot_exports.h"
#include "hsmp_native.h"
#include "ue4ss_reflect.h"
#include "ue4ss_reflect_names.h"
#include "ue4ss_pins.h"
namespace RC::Unreal { class UObject; struct FFrame; }
namespace
{
using namespace hsmp_box;
using Getter = void** (*)(void*);
using Locals = unsigned char** (*)(void*);
using ParmsSize = std::uint16_t* (*)(void*);
using Flags = bool (*)(const void*,std::uint64_t);
using World = void* (*)(const void*);
using Name = const std::uint64_t* (*)(const void*);
using Next = void* (*)(void*);
using Register = void (*)(std::function<void(RC::Unreal::UObject*,RC::Unreal::FFrame&,void*)>);
struct Api { Getter node{}, object{}, children{}; Locals locals{}; ParmsSize parms{}; Flags flags{};
    World world{}; Name name{}; Next next{}; Register pre{}, post{}; } api;
const HsmpReflect* vt{};
Scope enrolled{};
struct Function { Identity id{}; HsmpProp box{}, mesh{}; unsigned size{}; } targets[2];
Identity owner_function{};
HsmpProp owner_return{}, pawn_mesh{}, extent{};
unsigned owner_size{}, pawn_size{}, box_size{};
std::uint64_t object_property{}, struct_property{}, vector_name{};
Sink sink{};
bool resolved{}, submitted{}, uncertain{};
thread_local bool reading{};
const char* enrollment_stage="none";
FormalDiagnostic formal_detail{};
EnrollmentTiming enrollment_costs{};
void stage(const char* name,const char* failure="unavailable")
{
    if (enrollment_costs.active()) enrollment_costs.stage(name,GetTickCount64());
    enrollment_stage=name; formal_detail={}; formal_detail.failure=failure;
}
const EnrollmentTiming* enrollment_timing() { return &enrollment_costs; }
const char* enrollment_detail()
{
    static char detail[512]{};
    std::snprintf(detail,sizeof detail,
        "stage=%s failure=%s children=%d copied=%d visited=%u storage=%d copied_storage=%d parms=%u offset=%d size=%d matches=%u",
        enrollment_stage,formal_detail.failure,formal_detail.count,formal_detail.copied,formal_detail.visited,
        formal_detail.storage,formal_detail.copied_storage,formal_detail.parms_size,formal_detail.offset,formal_detail.size,formal_detail.matches);
    return detail;
}
std::uint64_t fname(const wchar_t* text) { return vt->fname(reinterpret_cast<const std::uint16_t*>(text),1); }
void* find(const wchar_t* text) { return vt->find(reinterpret_cast<const std::uint16_t*>(text)); }
bool pinned(HMODULE host)
{
    if (!host) return false;
    const auto* d=reinterpret_cast<const IMAGE_DOS_HEADER*>(host);
    if (d->e_magic != IMAGE_DOS_SIGNATURE) return false;
    const auto* n=reinterpret_cast<const IMAGE_NT_HEADERS64*>(reinterpret_cast<const unsigned char*>(host)+d->e_lfanew);
    return n->Signature == IMAGE_NT_SIGNATURE && n->FileHeader.TimeDateStamp == HSMP_UE4SS_TIMESTAMP
        && n->OptionalHeader.SizeOfImage == HSMP_UE4SS_SIZE_OF_IMAGE;
}
bool initialize()
{
    if (resolved) return true;
    vt=hsmp_reflect_table();
    HMODULE h=GetModuleHandleW(L"UE4SS.dll");
    if (!vt || !pinned(h)) return false; // Never honors the harness/unpinned override.
    void* exports[8]{};
    for (unsigned i=0;i<8;++i)
    { exports[i]=reinterpret_cast<void*>(GetProcAddress(h,kExports[i])); if (!exports[i]) return false; }
    api.node=reinterpret_cast<Getter>(exports[0]); api.object=reinterpret_cast<Getter>(exports[1]);
    api.locals=reinterpret_cast<Locals>(exports[2]); api.parms=reinterpret_cast<ParmsSize>(exports[3]);
    api.flags=reinterpret_cast<Flags>(exports[4]); api.world=reinterpret_cast<World>(exports[5]);
    api.pre=reinterpret_cast<Register>(exports[6]); api.post=reinterpret_cast<Register>(exports[7]);
    api.children=reinterpret_cast<Getter>(GetProcAddress(h,hsmp_reflect::kNames[5]));
    api.next=reinterpret_cast<Next>(GetProcAddress(h,hsmp_reflect::kNames[7]));
    api.name=reinterpret_cast<Name>(GetProcAddress(h,hsmp_reflect::kNames[17]));
    if (!api.children || !api.next || !api.name) return false;
    object_property=fname(L"ObjectProperty"); struct_property=fname(L"StructProperty"); vector_name=fname(L"Vector");
    resolved=object_property && struct_property && vector_name;
    return resolved;
}
Identity identity(void* p)
{
    Identity id{};
    if (!p) return id;
    id.weak=vt->weak(p);
    if (!id.weak || vt->resolve(id.weak) != p) return {};
    id.address=reinterpret_cast<std::uint64_t>(p); id.name=*api.name(p);
    void* c=vt->class_of(p);
    if (!c) return {};
    id.class_weak=vt->weak(c);
    if (!id.class_weak || vt->resolve(id.class_weak) != c) return {};
    id.class_address=reinterpret_cast<std::uint64_t>(c); id.class_name=*api.name(c);
    return id;
}
void* current(const Identity& id)
{
    // Resolve a slot BEFORE any dereference; serial-zero handles additionally pin address/name/class.
    if (!id.present()) return nullptr;
    void* p=vt->resolve(id.weak);
    if (reinterpret_cast<std::uint64_t>(p) != id.address) return nullptr;
    return identity(p) == id ? p : nullptr;
}
bool input_object(const ObjectInput& in,const wchar_t* cls,Identity& out)
{
    // A path lookup returns a live engine object. Never dereference the caller's numeric address.
    if (!in.path || in.path[0] != L'/' || !in.address) return false;
    void* p=find(in.path); void* c=find(cls);
    if (!p || reinterpret_cast<std::uint64_t>(p) != in.address || !c || !vt->is_a(p,c)) return false;
    out=identity(p); return out.present();
}
bool property(void* obj,const wchar_t* name,HsmpProp& p,unsigned& size)
{
    std::int32_t n{};
    if (!vt->obj_prop(obj,reinterpret_cast<const std::uint16_t*>(name),&p)) return false;
    vt->props(vt->class_of(obj),nullptr,0,&n);
    if (n <= 0 || n > 1024*1024 || p.offset < 0 || p.size <= 0 || p.offset > n || p.size > n-p.offset) return false;
    size=static_cast<unsigned>(n); return true;
}
bool formal(void* f,const wchar_t* name,HsmpProp& p,unsigned size,bool returns=false)
{
    struct Ops
    {
        std::int32_t props(void* object,HsmpProp* out,std::int32_t cap,std::int32_t* storage)
        { return vt->props(object,out,cap,storage); }
        void* first(void* object) { return *api.children(object); }
        void* next(void* field) { return api.next(field); }
        bool flags(void* field,std::uint64_t mask) { return api.flags(field,mask); }
    } ops;
    return read_formal(ops,f,fname(name),object_property,size,returns,p,formal_detail);
}
bool function(const wchar_t* path,Function& f,const wchar_t* mesh,
              const char* function_stage,const char* box_stage,const char* mesh_stage)
{
    stage(function_stage,"function_lookup");
    void* p=find(path); void* c=find(L"/Script/CoreUObject.Function");
    if (!p || !c || !vt->is_a(p,c)) return false;
    f.id=identity(p); auto* size=api.parms(p);
    formal_detail.failure="function_identity_or_size";
    if (!f.id.present() || !size || *size == 0) return false;
    f.size=*size;
    stage(box_stage);
    if (!formal(p,L"Hit Box",f.box,f.size)) return false;
    stage(mesh_stage);
    return formal(p,mesh,f.mesh,f.size);
}
void* object_at(void* object,const HsmpProp& field)
{
    void* p{}; std::memcpy(&p,static_cast<const unsigned char*>(object)+field.offset,sizeof p); return p;
}
void* owner(void* component)
{
    void* f=current(owner_function);
    if (!f) return nullptr;
    alignas(16) unsigned char params[64]{};
    vt->call(component,f,params);
    return object_at(params,owner_return);
}
bool scope_current(Reason& why)
{
    void* w=current(enrolled.world); void* p=current(enrolled.pawn); void* m=current(enrolled.mesh);
    void* b=current(enrolled.box); void* o=current(enrolled.box_owner);
    if (!w || !p || !m || !b || !o) { why=Reason::Identity; return false; }
    // The pinned UObject GetWorld wrapper is proven on actors. Components use
    // their exact current owner relationship below, not an assumed component getter.
    if (api.world(p)!=w || api.world(o)!=w)
    { why=Reason::World; return false; }
    if (object_at(p,pawn_mesh)!=m || owner(m)!=p || owner(b)!=o) { why=Reason::Scope; return false; }
    return true;
}
bool enroll(const Enrollment& in,Scope& out,Reason& why)
{
    enrollment_costs.begin();
    struct Finish { ~Finish() { enrollment_costs.finish(GetTickCount64()); } } finish;
    stage("initialize");
    if (!initialize()) { why=Reason::Unavailable; return false; }
    Scope s{}; s.match_id=in.match_id; s.round=in.round; s.life=in.life;
    stage("scope_identity");
    if (!input_object(in.world,L"/Script/Engine.World",s.world)
        || !input_object(in.pawn,L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C",s.pawn)
        || !input_object(in.mesh,L"/Script/Engine.SkeletalMeshComponent",s.mesh)
        || !input_object(in.box,L"/Script/Engine.BoxComponent",s.box)
        || !input_object(in.box_owner,L"/Script/Engine.Actor",s.box_owner) || !s.present())
    { why=Reason::Identity; return false; }
    auto* p=reinterpret_cast<void*>(s.pawn.address); auto* b=reinterpret_cast<void*>(s.box.address);
    stage("pawn_mesh");
    if (!property(p,L"Mesh",pawn_mesh,pawn_size) || pawn_mesh.cls!=object_property || pawn_mesh.size!=sizeof(void*))
    { why=Reason::Params; return false; }
    stage("box_extent");
    if (!property(b,L"BoxExtent",extent,box_size) || extent.cls!=struct_property
        || extent.sub!=vector_name || (extent.size!=12 && extent.size!=24))
    { why=Reason::Params; return false; }
    if (!function(L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage",targets[0],L"Hit Component",
                  "dcd_function","dcd_hit_box","dcd_hit_component")
        || !function(L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage",targets[1],L"Damaged Mesh",
                     "gd_function","gd_hit_box","gd_damaged_mesh"))
    { why=Reason::Params; return false; }
    stage("owner_function","function_lookup");
    void* own=find(L"/Script/Engine.ActorComponent:GetOwner");
    void* fn_class=find(L"/Script/CoreUObject.Function");
    if (!own || !fn_class || !vt->is_a(own,fn_class)) { why=Reason::Params; return false; }
    owner_function=identity(own); auto* os=own ? api.parms(own) : nullptr;
    formal_detail.failure="function_identity_or_size";
    if (!owner_function.present() || !os || *os==0 || *os>64) { why=Reason::Params; return false; }
    owner_size=*os;
    stage("owner_return");
    if (!formal(own,L"ReturnValue",owner_return,owner_size,true)) { why=Reason::Params; return false; }
    enrolled=s;
    stage("scope_current");
    if (!scope_current(why)) { enrolled={}; return false; }
    stage("complete","none");
    out=s; return true;
}
bool key(void* context,void* frame,Key& out,Reason& why)
{
    if (!resolved || !frame) return false;
    void** n=api.node(frame);
    if (!n || !*n) return false;
    const auto node=reinterpret_cast<std::uint64_t>(*n);
    const unsigned role=node==targets[0].id.address ? 1u : node==targets[1].id.address ? 2u : 0u;
    if (!role || reinterpret_cast<std::uint64_t>(context)!=enrolled.pawn.address) return false;
    void** o=api.object(frame);
    if (!o || *o!=context) { why=Reason::Params; return false; }
    out={reinterpret_cast<std::uint64_t>(frame),node,reinterpret_cast<std::uint64_t>(context),role};
    return true;
}
bool snapshot(const Key& k,void* frame,Snapshot& out,Reason& why)
{
    if (k.role<1 || k.role>2 || !scope_current(why)) return false;
    const auto& f=targets[k.role-1];
    if (!current(f.id) || k.node!=f.id.address || k.context!=enrolled.pawn.address)
    { why=Reason::Identity; return false; }
    unsigned char** local=api.locals(frame);
    if (!local || !*local || object_at(*local,f.box)!=reinterpret_cast<void*>(enrolled.box.address)
        || object_at(*local,f.mesh)!=reinterpret_cast<void*>(enrolled.mesh.address))
    { why=Reason::Params; return false; }
    // Raw formal values have only been compared; no un-enrolled pointer is dereferenced.
    void* b=current(enrolled.box);
    if (!b) { why=Reason::Identity; return false; }
    const auto* bytes=static_cast<const unsigned char*>(b)+extent.offset;
    Extent value{};
    if (extent.size==12) { float v[3]; std::memcpy(v,bytes,sizeof v); value={v[0],v[1],v[2]}; }
    else { double v[3]; std::memcpy(v,bytes,sizeof v); value={v[0],v[1],v[2]}; }
    if (!value.valid()) { why=Reason::Extent; return false; }
    out={enrolled,f.id,value,true,Reason::None}; return true;
}
bool submit(Sink callback,Reason& why)
{
    if (uncertain) { why=Reason::Submission; return false; }
    if (submitted) return sink==callback;
    if (!resolved || !callback) { why=Reason::Unavailable; return false; }
    uncertain=true; sink=callback;
    try
    {
        api.pre([](RC::Unreal::UObject* p,RC::Unreal::FFrame& f,void*) { if (sink && !reading) sink(1,p,&f); });
        api.post([](RC::Unreal::UObject* p,RC::Unreal::FFrame& f,void*) { if (sink && !reading) sink(2,p,&f); });
        submitted=true; uncertain=false; return true;
    }
    catch (...) { why=Reason::Submission; return false; }
}
bool safe_enroll(const Enrollment& in,Scope& out,Reason& why)
{
    reading=true; struct Guard { ~Guard() { reading=false; } } guard;
    return enroll(in,out,why);
}
bool safe_snapshot(const Key& k,void* frame,Snapshot& out,Reason& why)
{
    reading=true; struct Guard { ~Guard() { reading=false; } } guard;
    return snapshot(k,frame,out,why);
}
bool thread_ok() { return hsmp_native_caller_thread_ok()!=0; }
std::uint64_t now_ms() { return GetTickCount64(); }
const Provider provider{thread_ok,now_ms,safe_enroll,submit,key,safe_snapshot,enrollment_detail,enrollment_timing};
}
const hsmp_box::Provider& hsmp_reflect_box_provider() { return provider; }
