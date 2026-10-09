// Inert render replicas. All reflection is synchronous and game-thread guarded by Rust.
// Layouts are checked against reflected properties before any native memory read.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include "native_presentation.h"
#include "hsmp_native.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <map>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <tuple>
#include <type_traits>
#include <vector>

namespace {
const HsmpReflect* vt{};
DWORD game_thread{};
using Obj = HsmpViewObject;
using Transform = HsmpViewTransform;
using NamePrivate = const uint64_t*(*)(const void*);
NamePrivate object_name{};
using GetWorld = void*(*)(const void*);
GetWorld object_world{};
struct Identity {uint64_t address{},name{},class_weak{},class_address{};};
std::map<uint64_t,Identity> identities;
struct Error : std::runtime_error { using std::runtime_error::runtime_error; };
void require(bool ok, const char* why) { if (!ok) throw Error(why); }
thread_local const HsmpViewGuard* active_guard{};
thread_local Obj active_world{},active_game_instance{};
thread_local bool static_profile_trace{};
struct StaticProfileTraceScope {bool previous;StaticProfileTraceScope():previous(static_profile_trace){static_profile_trace=true;}~StaticProfileTraceScope(){static_profile_trace=previous;}};
void profile_phase(const char* stage,uint32_t edge){if(static_profile_trace)hsmp_native_profile_checkpoint(stage,edge);}
void profile_tick(uint32_t counter){if(static_profile_trace)hsmp_native_profile_tick(counter);}
void check_guard() {
    if(!active_guard)return;
    profile_tick(0);
    require(active_guard->check && active_guard->context && active_guard->check(active_guard->context)==1,
            "native source/world operation guard changed");
    if(active_game_instance.weak) {
        void* gi=vt->resolve(active_game_instance.weak);void* world=vt->resolve(active_world.weak);
        require(gi && reinterpret_cast<uint64_t>(gi)==active_game_instance.address && world &&
            reinterpret_cast<uint64_t>(world)==active_world.address,"native active world expired");
        const auto it=identities.find(active_game_instance.weak);
        require(it!=identities.end(),"native game-instance identity missing");
        const auto& id=it->second;void* cls=vt->resolve(id.class_weak);const auto n=object_name(gi);
        require(cls && reinterpret_cast<uint64_t>(cls)==id.class_address && vt->class_of(gi)==cls && n && *n==id.name,
            "native game-instance identity changed");
        require(object_world && object_world(gi)==world,"native game-instance world changed");
    }
}
void thread() {
    require(vt && vt->abi == HSMP_REFLECT_ABI, "reflection unavailable");
    require(object_name!=nullptr,"native object-name provider unavailable");
    const DWORD current = GetCurrentThreadId();
    if (!game_thread) game_thread = current; // only entered through Rust's game-thread API
    require(current == game_thread, "presentation game thread changed");
}
const uint16_t* u16(const wchar_t* s) { return reinterpret_cast<const uint16_t*>(s); }
std::map<std::wstring,uint64_t> names;
uint64_t name(const wchar_t* s) {
    const auto it=names.find(s);if(it!=names.end())return it->second;
    require(names.size()<65536,"presentation FName cache bounds");
    check_guard();const auto value=vt->fname(u16(s),1);names.emplace(s,value);return value;
}
std::wstring text(HsmpViewText s) {
    require(s.len <= 1024 && (s.len == 0 || s.data), "presentation text bounds");
    if (!s.len) return {};
    std::wstring out(reinterpret_cast<const wchar_t*>(s.data), s.len);
    require(out.find(L'\0') == std::wstring::npos, "presentation text NUL");
    return out;
}
uint64_t name(HsmpViewText s) { const auto str = text(s); return name(str.c_str()); }
Obj keep(void* p) {
    check_guard();
    require(p != nullptr, "native object unavailable");
    const uint64_t weak = vt->weak(p);
    require(weak && vt->resolve(weak) == p, "native weak identity unavailable");
    void* cls=vt->class_of(p);const auto class_weak=cls?vt->weak(cls):0;
    require(class_weak && vt->resolve(class_weak)==cls,"native class identity unavailable");
    const auto object_name_ptr=object_name(p);require(object_name_ptr!=nullptr,"native object name unavailable");
    const Identity id{reinterpret_cast<uint64_t>(p),*object_name_ptr,class_weak,reinterpret_cast<uint64_t>(cls)};
    const auto existing=identities.find(weak);
    if(existing!=identities.end()) require(existing->second.address==id.address && existing->second.name==id.name &&
        existing->second.class_weak==id.class_weak && existing->second.class_address==id.class_address,"native serial-zero identity reused");
    else {require(identities.size()<65536,"native identity cache bounds");identities.emplace(weak,id);}
    return {weak, reinterpret_cast<uint64_t>(p)};
}
void* get(Obj o) {
    check_guard();
    void* p = vt->resolve(o.weak);
    require(o.weak && p && reinterpret_cast<uint64_t>(p) == o.address, "native object expired");
    const auto existing=identities.find(o.weak);
    if(existing==identities.end()) {keep(p);}
    else {
        const auto& id=existing->second;require(id.address==o.address,"native identity address changed");
        void* cls=vt->resolve(id.class_weak);const auto n=object_name(p);
        require(cls && reinterpret_cast<uint64_t>(cls)==id.class_address && vt->class_of(p)==cls && n && *n==id.name,
            "native object name/class changed");
    }
    return p;
}
Obj find(const wchar_t* path) {check_guard();profile_tick(2);return keep(vt->find(u16(path)));}
bool is(Obj o, const wchar_t* cls) { const auto c = find(cls); return vt->is_a(get(o), get(c)) != 0; }
Obj asset(HsmpViewText path, const wchar_t* cls) {
    const auto p = text(path);
    require(!p.empty() && p[0] == L'/', "native asset path missing");
    Obj o = find(p.c_str()); require(is(o, cls), "native asset class mismatch"); return o;
}
bool same(Obj a, Obj b) { return a.weak == b.weak && a.address == b.address; }
HsmpProp property(Obj o, const wchar_t* key, const wchar_t* type, int size) {
    HsmpProp p{};
    require(vt->obj_prop(get(o), u16(key), &p) == 1, "native object property missing");
    require(p.cls == name(type) && p.size == size && p.offset >= 0 && p.offset < 65536,
            "native object property layout");
    return p;
}
template<class T> T read(Obj o, const wchar_t* key, const wchar_t* type) {
    auto p = property(o, key, type, static_cast<int>(sizeof(T)));
    T value{}; std::memcpy(&value, static_cast<uint8_t*>(get(o)) + p.offset, sizeof(T));
    get(o); return value;
}
Obj object_property(Obj o, const wchar_t* key) {
    void* p = read<void*>(o, key, L"ObjectProperty"); return p ? keep(p) : Obj{};
}
struct OperationScope {
    const HsmpViewGuard* previous{};Obj previous_world{},previous_gi{};
    explicit OperationScope(const HsmpViewGuard* guard,Obj world):previous(active_guard),previous_world(active_world),previous_gi(active_game_instance) {
        require(guard && guard->context && guard->check && guard->check(guard->context)==1,"native borrowed guard missing");
        active_guard=guard;active_world={};active_game_instance={};
        try {
            get(world);require(is(world,L"/Script/Engine.World"),"native guard world class");
            auto gi=object_property(world,L"OwningGameInstance");require(gi.weak&&is(gi,L"/Script/Engine.GameInstance"),"native owning game-instance missing");
            active_world=world;active_game_instance=gi;check_guard();
        }catch(...) {active_guard=previous;active_world=previous_world;active_game_instance=previous_gi;throw;}
    }
    ~OperationScope(){active_guard=previous;active_world=previous_world;active_game_instance=previous_gi;}
};
bool bool_property(Obj o, const wchar_t* key) {
    auto p = property(o, key, L"BoolProperty", 1);
    require(p.bool_mask != 0, "native bool mask missing");
    const uint8_t value = *(static_cast<uint8_t*>(get(o)) + p.offset + p.bool_offset);
    get(o); return (value & p.bool_mask) != 0;
}
struct Array { void* data; int32_t count, capacity; };
static_assert(sizeof(Array) == 16);
struct Signature {Obj function{},cls{};std::vector<HsmpProp> fields;};
std::map<std::wstring,Signature> signatures;
void spline_call_guard(Obj object,Obj function,Obj cls);
void vertex_call_guard(Obj object,Obj function,Obj cls);
void vertex_dispatch_guard(Obj object,Obj function,Obj cls);
void scene_call_guard(Obj object,Obj function,Obj cls);
void scene_dispatch_guard(Obj object,Obj function,Obj cls);

struct Function {
    Obj function{}, cls{};
    std::vector<HsmpProp> fields;
    uint64_t alignment_pad{};
    alignas(16) std::array<uint8_t,4096> buf{};
    Function(const wchar_t* path) {
        auto existing=signatures.find(path);
        if(existing!=signatures.end()) {
            get(existing->second.function);get(existing->second.cls);
            function=existing->second.function;cls=existing->second.cls;fields=existing->second.fields;return;
        }
        require(signatures.size()<128,"presentation function cache bounds");
        function = find(path);
        std::wstring class_path(path); const auto colon = class_path.find(L':');
        require(colon != std::wstring::npos, "native function path");
        class_path.resize(colon); cls = find(class_path.c_str());
        std::array<HsmpProp,64> properties{}; int32_t size{};
        const int32_t n = vt->props(get(function), properties.data(), 64, &size);
        require(n >= 0 && n <= 64 && size >= 0 && size <= 4096, "native function bounds");
        fields.assign(properties.begin(), properties.begin() + n);
        for (const auto& p : fields) require(p.offset >= 0 && p.size > 0 &&
            p.offset + p.size <= size, "native function field bounds");
        signatures.emplace(path,Signature{function,cls,fields});
    }
    HsmpProp field(const wchar_t* key, const wchar_t* type, int size, const wchar_t* sub = nullptr) const {
        const auto n = name(key);
        for (auto p : fields) if (p.name == n) {
            require(p.cls == name(type) && p.size == size && (!sub || p.sub == name(sub)),
                    "native function signature mismatch"); return p;
        }
        throw Error("native function parameter missing");
    }
    template<class T> void put(const wchar_t* key, const wchar_t* type, const T& value, const wchar_t* sub = nullptr) {
        const auto p = field(key, type, static_cast<int>(sizeof(T)), sub);
        std::memcpy(buf.data() + p.offset, &value, sizeof(T));
    }
    template<class T> T value(const wchar_t* key, const wchar_t* type, const wchar_t* sub = nullptr) const {
        T out{}; const auto p = field(key, type, static_cast<int>(sizeof(T)), sub);
        std::memcpy(&out, buf.data() + p.offset, sizeof(T)); return out;
    }
    void boolean(const wchar_t* key, bool value) {
        auto p = field(key, L"BoolProperty", 1);
        require(p.bool_mask && p.offset + p.bool_offset < static_cast<int>(buf.size()), "native bool signature");
        auto& b = buf[static_cast<size_t>(p.offset + p.bool_offset)];
        b = static_cast<uint8_t>((b & ~p.bool_mask) | (value ? p.bool_mask : 0));
    }
    void enumeration(const wchar_t* key, uint8_t value) {
        const auto n = name(key);
        for (auto p : fields) if (p.name == n) {
            require(p.size == 1 && (p.cls == name(L"ByteProperty") || p.cls == name(L"EnumProperty")),
                    "native enum signature"); buf[static_cast<size_t>(p.offset)] = value; return;
        }
        throw Error("native enum missing");
    }
    void object(const wchar_t* key, Obj obj, bool class_parameter = false) {
        void* p = obj.weak ? get(obj) : nullptr;
        put(key, class_parameter ? L"ClassProperty" : L"ObjectProperty", p);
    }
    void call(Obj object, HsmpViewResult* result = nullptr) {
        spline_call_guard(object,function,cls);
        vertex_call_guard(object,function,cls);
        scene_call_guard(object,function,cls);
        require(vt->is_a(get(object), get(cls)) != 0, "native function owner class mismatch");
        void* object_pointer=get(object);void* function_pointer=get(function);
        vertex_dispatch_guard(object,function,cls);
        scene_dispatch_guard(object,function,cls);
        profile_tick(3);profile_phase("cpp_pe",0);
        vt->call(object_pointer,function_pointer,buf.data());
        profile_phase("cpp_pe",1);
        check_guard();
        spline_call_guard(object,function,cls);
        vertex_call_guard(object,function,cls);
        scene_call_guard(object,function,cls);
        get(object); get(function); get(cls); if (result) ++result->operations;
    }
    Obj returned() const { auto p = value<void*>(L"ReturnValue", L"ObjectProperty"); return p ? keep(p) : Obj{}; }
};
// Engine transform includes two padding words; the wire transform deliberately does not.
struct alignas(16) EngineTransform { double q[4]; double p[3]; double pad_p{}; double scale[3]; double pad_s{}; };
static_assert(sizeof(EngineTransform) == 96);
EngineTransform engine(const Transform& t) {
    EngineTransform out{};
    for (double x : t.p) require(std::isfinite(x), "native translation invalid");
    for (double x : t.q) require(std::isfinite(x), "native quaternion invalid");
    for (double x : t.scale) require(std::isfinite(x), "native scale invalid");
    std::copy_n(t.q,4,out.q); std::copy_n(t.p,3,out.p); std::copy_n(t.scale,3,out.scale); return out;
}
Transform wire(const EngineTransform& t) {
    Transform out{}; std::copy_n(t.p,3,out.p); std::copy_n(t.q,4,out.q); std::copy_n(t.scale,3,out.scale);
    engine(out); return out;
}
std::vector<Obj> layout_objects;
bool layouts_verified{};
void layout(const wchar_t* path, int size, std::initializer_list<std::tuple<const wchar_t*,const wchar_t*,int,int,const wchar_t*>> expected) {
    const auto object = find(path); std::array<HsmpProp,16> props{}; int32_t actual_size{};
    const int32_t count = vt->props(get(object), props.data(), 16, &actual_size);
    require(count >= 0 && count <= 16 && actual_size == size, "native struct size mismatch");
    for (const auto& [key,type,offset,bytes,sub] : expected) {
        bool found{}; for (int i=0; i<count; ++i) {
            const auto& p=props[static_cast<size_t>(i)];
            if (p.name == name(key)) { require(p.cls == name(type) && p.offset == offset && p.size == bytes &&
                (!sub || p.sub == name(sub)), "native struct field mismatch"); found=true; }
        } require(found,"native struct field missing");
    }
    layout_objects.push_back(object);
}
void layouts() {
    if(layouts_verified){for(auto object:layout_objects)get(object);return;}
    layout_objects.clear();
    layout(L"/Script/CoreUObject.Vector",24,{{L"X",L"DoubleProperty",0,8,nullptr},{L"Y",L"DoubleProperty",8,8,nullptr},{L"Z",L"DoubleProperty",16,8,nullptr}});
    layout(L"/Script/CoreUObject.Quat",32,{{L"X",L"DoubleProperty",0,8,nullptr},{L"Y",L"DoubleProperty",8,8,nullptr},{L"Z",L"DoubleProperty",16,8,nullptr},{L"W",L"DoubleProperty",24,8,nullptr}});
    layout(L"/Script/CoreUObject.Transform",96,{{L"Rotation",L"StructProperty",0,32,L"Quat"},{L"Translation",L"StructProperty",32,24,L"Vector"},{L"Scale3D",L"StructProperty",64,24,L"Vector"}});
    layout(L"/Script/CoreUObject.LinearColor",16,{{L"R",L"FloatProperty",0,4,nullptr},{L"G",L"FloatProperty",4,4,nullptr},{L"B",L"FloatProperty",8,4,nullptr},{L"A",L"FloatProperty",12,4,nullptr}});
    layout(L"/Script/CoreUObject.Color",4,{{L"B",L"ByteProperty",0,1,nullptr},{L"G",L"ByteProperty",1,1,nullptr},{L"R",L"ByteProperty",2,1,nullptr},{L"A",L"ByteProperty",3,1,nullptr}});
    layout(L"/Script/Engine.MaterialParameterInfo",16,{{L"Name",L"NameProperty",0,8,nullptr},{L"Association",L"ByteProperty",8,1,nullptr},{L"Index",L"IntProperty",12,4,nullptr}});
    layouts_verified=true;
}
Obj returned(Obj object, const wchar_t* path, HsmpViewResult* r = nullptr) { Function f(path); f.call(object,r); return f.returned(); }
Obj actor_world(Obj actor, HsmpViewResult* r = nullptr) {
    require(is(actor,L"/Script/Engine.Actor"), "presentation actor class");
    auto level = returned(actor,L"/Script/Engine.Actor:GetLevel",r);
    require(level.weak && is(level,L"/Script/Engine.Level"),"native actor level missing");
    auto world = object_property(level,L"OwningWorld"); require(world.weak && is(world,L"/Script/Engine.World"),"native owning world missing"); return world;
}
void qualify(Obj world, Obj owner, Obj component, HsmpViewResult* r = nullptr) {
    get(world); require(is(component,L"/Script/Engine.SceneComponent"),"native component class");
    const auto actual_owner=returned(component,L"/Script/Engine.ActorComponent:GetOwner",r);
    require(same(actual_owner,owner) && same(actor_world(owner,r),world),"native component owner/world mismatch");
}
Obj mesh_asset(Obj component, HsmpViewResult* r) { return returned(component,L"/Script/Engine.SkinnedMeshComponent:GetSkinnedAsset",r); }
bool effective_visible(Obj component) {return bool_property(component,L"bVisible")&&!bool_property(component,L"bHiddenInGame");}
void visibility(Obj component,bool visible,HsmpViewResult* r);
void collision_off(Obj component,HsmpViewResult* r);
#include "native_spline_impl.h"
#include "native_vertex_state_impl.h"
bool close(const Transform&,const Transform&);
#include "native_scene_impl.h"
void scene_anchor(Obj component) {
    const auto cls=keep(vt->class_of(get(component)));
    if(same(cls,find(L"/Script/Engine.CameraComponent"))){scene_profile(component,6);return;}
    if(same(cls,find(L"/Script/Engine.SpringArmComponent"))){scene_profile(component,7);return;}
    if(same(cls,find(L"/Script/Engine.SceneComponent")))return;
    if(same(cls,find(L"/Script/Engine.SplineComponent"))) {
        require(!bool_property(component,L"bDrawDebug"),"native spline own rendering unsupported");return;
    }
    if(same(cls,find(L"/Script/Engine.CapsuleComponent"))) {
        require(!effective_visible(component),"native visible capsule own rendering unsupported");return;
    }
    throw Error("native scene anchor class unsupported");
}
void supported(Obj world,Obj owner,Obj component,HsmpViewResult* r) {
    qualify(world,owner,component,r);
    if (is(component,L"/Script/Engine.SkinnedMeshComponent")) {
        auto mesh=mesh_asset(component,r); require(mesh.weak && is(mesh,L"/Script/Engine.SkeletalMesh"),"native skeletal asset missing");
        const auto clothing=read<Array>(mesh,L"MeshClothingAssets",L"ArrayProperty");
        require(clothing.count==0 && clothing.capacity>=0,"native cloth requires source surface replication");
        require(!object_property(mesh,L"DefaultMeshDeformer").weak && !object_property(component,L"MeshDeformer").weak,
                "native deformer requires source surface replication");
        require(!bool_property(component,L"bSetMeshDeformer"),"native deformer override unsupported");
        auto deformer=returned(component,L"/Script/Engine.SkinnedMeshComponent:GetMeshDeformerInstance",r);
        require(!deformer.weak,"native deformer instance unsupported");
    } else if(is(component,L"/Script/Engine.SplineComponent"))spline_class(component);
    else if(!is(component,L"/Script/Engine.StaticMeshComponent"))scene_anchor(component);
    qualify(world,owner,component,r);
}
bool pointers(const void* p,uint32_t n,uint32_t cap) { return n<=cap && (!n||p); }
void recipe(const HsmpViewComponent& c) {
    require((c.kind<=1||(c.kind>=4&&c.kind<=8)) && c.visible<=1 && pointers(c.bones,c.bone_count,512) && pointers(c.morphs,c.morph_count,128) &&
        pointers(c.hidden_bones,c.hidden_count,512) && pointers(c.materials,c.material_count,32) &&
        pointers(c.vertex_lods,c.vertex_count,16),"native component recipe bounds");
    if(c.kind>=4&&c.kind<=7) {
        require(c.vertex_state==4&&!c.bone_count&&!c.morph_count&&!c.hidden_count&&!c.material_count&&!c.vertex_count,
                "native scene anchor render dictionary");
        const auto cls=text(c.asset);
        require((c.kind==6&&cls==L"/Script/Engine.CameraComponent")||(c.kind==7&&cls==L"/Script/Engine.SpringArmComponent")||
            ((c.kind==4||c.kind==5)&&(cls==L"/Script/Engine.SceneComponent"||cls==L"/Script/Engine.SplineComponent"||cls==L"/Script/Engine.CapsuleComponent")),
                "native scene anchor recipe class");
        require(cls!=L"/Script/Engine.CapsuleComponent"||c.visible==0,"native scene anchor capsule visibility");
        require(text(c.skeleton).empty(),"native scene anchor skeletal asset");
        if(c.kind==5){require(cls==L"/Script/Engine.SplineComponent","native spline recipe exact class");spline_profile_valid(c.spline);}
    }else if(c.kind==8){require(text(c.asset)==L"/Script/Engine.StaticMeshComponent"&&text(c.skeleton).empty()&&c.vertex_state==4&&!c.vertex_count,"native empty static recipe");}
    else require(c.vertex_state==0 || c.vertex_state==1,"native vertex recipe incomplete");
    if(c.kind!=5)require(c.spline.position_count==0&&c.spline.rotation_count==0&&c.spline.scale_count==0&&c.spline.reparam_count==0&&c.spline.metadata_null==0,"native non-spline profile present");
    require((c.kind==7)==(c.spring_arm_socket.len>0),"native spring arm singleton recipe presence");
    if(c.kind==7){const auto s=text(c.spring_arm_socket);require(s.size()<=128&&s!=L"None","native spring arm socket recipe");}
    require(c.vertex_state!=1 || c.vertex_count>0,"native vertex colors missing");
    require(c.kind==0 || (!c.bone_count&&!c.morph_count&&!c.hidden_count),"static component skeletal dictionary");
    engine(c.relative); for(uint32_t i=0;i<c.material_count;++i) {
        const auto& m=c.materials[i]; require(m.slot<32 && pointers(m.scalars,m.scalar_count,128) &&
            pointers(m.vectors,m.vector_count,128) && pointers(m.textures,m.texture_count,128),"native material dictionary bounds");
    }
    for(uint32_t i=0;i<c.vertex_count;++i) {
        const auto& l=c.vertex_lods[i]; require(l.lod<16 && l.count>0 && l.count<=1000000 && l.bytes==l.count*4 && l.rgba,
            "native vertex color bounds");
    }
}
// Preserve direct parent links and apply parents before children. Setting a
// parent later would move already-applied children away from their source pose.
std::vector<uint32_t> parent_order(const HsmpViewComponent* components,uint32_t count) {
    require(pointers(components,count,64)&&count>0,"native parent graph bounds");
    std::map<uint32_t,uint32_t> indices;
    for(uint32_t i=0;i<count;++i)require(components[i].id!=0&&indices.emplace(components[i].id,i).second,"native parent graph duplicate id");
    std::vector<uint8_t> state(count);std::vector<uint32_t> order;order.reserve(count);
    const auto visit=[&](const auto& self,uint32_t i)->void {
        require(state[i]!=1,"native parent graph cycle");if(state[i]==2)return;state[i]=1;
        if(components[i].parent) {
            const auto parent=indices.find(components[i].parent);require(parent!=indices.end(),"native parent graph missing node");
            self(self,parent->second);
        }
        state[i]=2;order.push_back(i);
    };
    for(uint32_t i=0;i<count;++i)visit(visit,i);
    return order;
}
void frame(const HsmpViewComponent& c,const HsmpViewFrame& f) {
    recipe(c); uint32_t ns{},nv{},nt{};
    for(uint32_t i=0;i<c.material_count;++i) {ns+=c.materials[i].scalar_count;nv+=c.materials[i].vector_count;nt+=c.materials[i].texture_count;}
    require(f.bone_count==c.bone_count && f.morph_count==c.morph_count && f.scalar_count==ns && f.vector_count==nv && f.texture_count==nt &&
        pointers(f.bones,f.bone_count,512) && pointers(f.morphs,f.morph_count,128) && pointers(f.scalars,ns,4096) &&
        pointers(f.vectors,nv,4096) && pointers(f.textures,nt,4096),"native frame dictionary mismatch"); engine(f.world);
    require((c.kind==5)==(f.spline!=nullptr),"native spline frame presence");if(f.spline)spline_frame_valid(c.spline,*f.spline);
    require((c.kind==7)==(f.spring_arm!=nullptr),"native spring arm frame presence");if(f.spring_arm)arm_valid(*f.spring_arm);
}
Transform transform(Obj component,const wchar_t* path,HsmpViewResult* r) {
    Function f(path); f.call(component,r); return wire(f.value<EngineTransform>(L"ReturnValue",L"StructProperty",L"Transform"));
}
Transform bone(Obj component,HsmpViewText b,HsmpViewResult* r) {
    Function f(L"/Script/Engine.SceneComponent:GetSocketTransform"); f.put(L"InSocketName",L"NameProperty",name(b));
    f.enumeration(L"TransformSpace",2); f.call(component,r); return wire(f.value<EngineTransform>(L"ReturnValue",L"StructProperty",L"Transform"));
}
Obj material(Obj component,uint32_t slot,HsmpViewResult* r) {
    Function f(L"/Script/Engine.PrimitiveComponent:GetMaterial"); f.put(L"ElementIndex",L"IntProperty",static_cast<int32_t>(slot));
    f.call(component,r); auto out=f.returned(); require(out.weak,"native material slot unavailable"); return out;
}
void source_static(Obj component,const HsmpViewComponent& c,HsmpViewResult* r) {
    const bool visible=effective_visible(component);
    require(visible==(c.visible!=0),"source visibility recipe changed");
    for(uint32_t i=0;i<c.material_count;++i) {
        auto current=material(component,c.materials[i].slot,r);const auto expected=asset(c.materials[i].base,L"/Script/Engine.MaterialInterface");
        bool matched{};for(unsigned depth=0;depth<32;++depth) {
            if(same(current,expected)){matched=true;break;}
            if(!is(current,L"/Script/Engine.MaterialInstance"))break;
            const auto parent=object_property(current,L"Parent");if(!parent.weak||same(parent,current))break;current=parent;
        }require(matched,"source material base recipe changed");
    }
    if(c.kind==0) {
        Function count(L"/Script/Engine.SkinnedMeshComponent:GetNumBones");count.call(component,r);
        require(count.value<int32_t>(L"ReturnValue",L"IntProperty")==static_cast<int32_t>(c.bone_count),"source render bone dictionary changed");
        for(uint32_t i=0;i<c.bone_count;++i) {
            const auto id=name(c.bones[i]);bool expected{};for(uint32_t j=0;j<c.hidden_count;++j)if(name(c.hidden_bones[j])==id){expected=true;break;}
            Function hidden(L"/Script/Engine.SkinnedMeshComponent:IsBoneHiddenByName");hidden.put(L"BoneName",L"NameProperty",id);hidden.call(component,r);
            auto p=hidden.field(L"ReturnValue",L"BoolProperty",1);const bool actual=(hidden.buf[static_cast<size_t>(p.offset+p.bool_offset)]&p.bool_mask)!=0;
            require(actual==expected,"source hidden bone recipe changed");
        }
    }
}
struct ParameterInfo {uint64_t name;uint8_t association;uint8_t pad[3];int32_t index;};
static_assert(sizeof(ParameterInfo)==16);
ParameterInfo parameter(const HsmpViewParameter& p) {
    require(p.association<=2 && p.index>=-1,"native material parameter info");
    return {name(p.name),static_cast<uint8_t>(p.association),{},p.index};
}
template<class T> T material_get(Obj material,const HsmpViewParameter& p,const wchar_t* path,const wchar_t* type,const wchar_t* sub,HsmpViewResult* r) {
    const bool dynamic=is(material,L"/Script/Engine.MaterialInstanceDynamic");
    std::wstring actual(path);
    if(!dynamic) {
        require(is(material,L"/Script/Engine.MaterialInstanceConstant") && p.association==0 && p.index==-1,
                "native layered material getter unavailable");
        const auto marker=actual.find(L"MaterialInstanceDynamic:");require(marker!=std::wstring::npos,"native material getter path");
        actual.replace(marker,24,L"MaterialInstanceConstant:");
        require(actual.size()>=6 && actual.substr(actual.size()-6)==L"ByInfo","native material getter suffix");actual.resize(actual.size()-6);
    }
    Function f(actual.c_str());
    if(dynamic)f.put(L"ParameterInfo",L"StructProperty",parameter(p),L"MaterialParameterInfo");
    else f.put(L"ParameterName",L"NameProperty",name(p.name));
    f.call(material,r); return f.value<T>(L"ReturnValue",type,sub);
}
void capture_values(Obj component,const HsmpViewComponent& c,HsmpViewFrame& out,HsmpViewResult* r) {
    out.world=transform(component,L"/Script/Engine.SceneComponent:K2_GetComponentToWorld",r);
    for(uint32_t i=0;i<c.bone_count;++i) out.bones[i]=bone(component,c.bones[i],r);
    for(uint32_t i=0;i<c.morph_count;++i) {
        Function f(L"/Script/Engine.SkeletalMeshComponent:GetMorphTarget"); f.put(L"MorphTargetName",L"NameProperty",name(c.morphs[i]));
        f.call(component,r); out.morphs[i]=f.value<float>(L"ReturnValue",L"FloatProperty");
    }
    uint32_t si{},vi{},ti{};
    for(uint32_t i=0;i<c.material_count;++i) {
        const auto& m=c.materials[i]; auto mat=material(component,m.slot,r);
        for(uint32_t j=0;j<m.scalar_count;++j) out.scalars[si++]=material_get<float>(mat,m.scalars[j],L"/Script/Engine.MaterialInstanceDynamic:K2_GetScalarParameterValueByInfo",L"FloatProperty",nullptr,r);
        for(uint32_t j=0;j<m.vector_count;++j) {
            auto v=material_get<std::array<float,4>>(mat,m.vectors[j],L"/Script/Engine.MaterialInstanceDynamic:K2_GetVectorParameterValueByInfo",L"StructProperty",L"LinearColor",r);
            std::copy(v.begin(),v.end(),out.vectors+4*vi++);
        }
        for(uint32_t j=0;j<m.texture_count;++j) {
            auto p=material_get<void*>(mat,m.textures[j],L"/Script/Engine.MaterialInstanceDynamic:K2_GetTextureParameterValueByInfo",L"ObjectProperty",nullptr,r);
            out.textures[ti++]=p?keep(p):Obj{};
        }
    }
}
using Free = void(*)(void*);
struct OwnedColorArray {
    Free release{};const uint8_t* result{};
    ~OwnedColorArray(){Array a{};std::memcpy(&a,result,sizeof(a));if(a.data)release(a.data);}
};
Free allocator() {
    const auto module=GetModuleHandleW(L"UE4SS.dll");
    auto free=reinterpret_cast<Free>(module?GetProcAddress(module,"?Free@FMemory@Unreal@RC@@SAXPEAX@Z"):nullptr);
    require(free!=nullptr,"native color allocator unavailable"); return free;
}
Obj paint_library() { return find(L"/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary"); }
void verify_colors(Obj component,const HsmpViewComponent& c,HsmpViewResult* r) {
    auto free=allocator();
    for(uint32_t i=0;i<c.vertex_count;++i) {
        const auto& lod=c.vertex_lods[i];
        Function f(L"/Script/VertexPaintDetectionPlugin.VertexPaintFunctionLibrary:GetMeshComponentVertexColorsAtLOD_Wrapper");
        f.object(L"MeshComponent",component); f.put(L"Lod",L"IntProperty",static_cast<int32_t>(lod.lod));
        const auto return_field=f.field(L"ReturnValue",L"ArrayProperty",16);
        OwnedColorArray owned{free,f.buf.data()+return_field.offset};f.call(paint_library(),r);
        const auto a=f.value<Array>(L"ReturnValue",L"ArrayProperty");
        bool ok=a.count==static_cast<int32_t>(lod.count) && a.capacity>=a.count && a.count>0 && a.data;
        if(ok) {const auto* bytes=static_cast<const uint8_t*>(a.data);for(uint32_t n=0;n<lod.count;++n) {
            const auto* b=bytes+4*n;const auto* expected=lod.rgba+4*n;
            if(b[2]!=expected[0]||b[1]!=expected[1]||b[0]!=expected[2]||b[3]!=expected[3]) {ok=false;break;}
        }}
        require(ok,"native vertex color readback mismatch");
    }
}
void colors(Obj component,const HsmpViewComponent& c,HsmpViewResult* r) {
    if(!c.vertex_count) return;
    // Cooked colors that already match the observed source need no override, including
    // static weapon meshes. Runtime-painted static meshes remain explicitly unsupported.
    try {verify_colors(component,c,r);return;}
    catch(const Error& e) {if(std::strcmp(e.what(),"native vertex color readback mismatch")!=0)throw;}
    // Painted static meshes need their own exact native setter; no invented default colors.
    require(c.kind==0,"painted static mesh setter unavailable");
    std::map<uint32_t,std::array<float,4>> palette;
    for(uint32_t i=0;i<c.vertex_count;++i) {
        const auto& lod=c.vertex_lods[i]; std::vector<std::array<float,4>> linear;linear.reserve(lod.count);
        for(uint32_t j=0;j<lod.count;++j) {
            const auto* rgba=lod.rgba+4*j;uint32_t key{};std::memcpy(&key,rgba,4);
            auto it=palette.find(key);if(it==palette.end()) {
                require(palette.size()<65536,"native color palette bounds");
                Function f(L"/Script/VertexPaintDetectionPlugin.VertexPaintFunctionLibrary:ReliableFColorToFLinearColor");
                std::array<uint8_t,4> bgra={rgba[2],rgba[1],rgba[0],rgba[3]};f.put(L"Color",L"StructProperty",bgra,L"Color");
                f.call(paint_library(),r);auto value=f.value<std::array<float,4>>(L"ReturnValue",L"StructProperty",L"LinearColor");
                it=palette.emplace(key,value).first;
            }linear.push_back(it->second);
        }
        Array a{linear.data(),static_cast<int32_t>(linear.size()),static_cast<int32_t>(linear.size())};
        Function f(L"/Script/Engine.SkinnedMeshComponent:SetVertexColorOverride_LinearColor");
        f.put(L"LODIndex",L"IntProperty",static_cast<int32_t>(lod.lod));f.put(L"VertexColors",L"ArrayProperty",a);f.call(component,r);
    }verify_colors(component,c,r);
}
void collision_off(Obj component,HsmpViewResult* r) {
    if(!is(component,L"/Script/Engine.PrimitiveComponent")) {
        const auto cls=keep(vt->class_of(get(component)));
        if(!same(cls,find(L"/Script/Engine.SceneComponent"))){const auto kind=scene_kind(component);require(kind==6||kind==7,"mirror nonprimitive class unsupported");scene_profile(component,kind);}
        Function tick(L"/Script/Engine.ActorComponent:SetComponentTickEnabled");tick.boolean(L"bEnabled",false);tick.call(component,r);
        Function read_tick(L"/Script/Engine.ActorComponent:IsComponentTickEnabled");read_tick.call(component,r);
        const auto tp=read_tick.field(L"ReturnValue",L"BoolProperty",1);
        require((read_tick.buf[static_cast<size_t>(tp.offset+tp.bool_offset)]&tp.bool_mask)==0,"mirror scene anchor tick still enabled");return;
    }
    Function physics(L"/Script/Engine.PrimitiveComponent:SetSimulatePhysics");physics.boolean(L"bSimulate",false);physics.call(component,r);
    Function collision(L"/Script/Engine.PrimitiveComponent:SetCollisionEnabled");collision.enumeration(L"NewType",0);collision.call(component,r);
    Function read_collision(L"/Script/Engine.PrimitiveComponent:GetCollisionEnabled");read_collision.call(component,r);
    const auto p=read_collision.fields;bool zero{};for(const auto& v:p) if(v.name==name(L"ReturnValue")) {
        require(v.size==1&&(v.cls==name(L"ByteProperty")||v.cls==name(L"EnumProperty")),"collision readback signature");zero=read_collision.buf[static_cast<size_t>(v.offset)]==0;
    }require(zero,"mirror collision still enabled");
    Function sim(L"/Script/Engine.SceneComponent:IsSimulatingPhysics");sim.put(L"BoneName",L"NameProperty",uint64_t{});sim.call(component,r);
    auto result=sim.field(L"ReturnValue",L"BoolProperty",1);require((sim.buf[static_cast<size_t>(result.offset+result.bool_offset)]&result.bool_mask)==0,"mirror physics still enabled");
}
void visibility(Obj component,bool visible,HsmpViewResult* r) {
    Function f(L"/Script/Engine.SceneComponent:SetVisibility");f.boolean(L"bNewVisibility",visible);f.boolean(L"bPropagateToChildren",false);f.call(component,r);
}
Obj add_component(Obj actor,const wchar_t* cls,const Transform& relative,HsmpViewResult* r) {
    Function f(L"/Script/Engine.Actor:AddComponentByClass");f.object(L"Class",find(cls),true);f.boolean(L"bManualAttachment",true);
    f.put(L"RelativeTransform",L"StructProperty",engine(relative),L"Transform");f.boolean(L"bDeferredFinish",true);f.call(actor,r);
    auto component=f.returned();require(component.weak&&is(component,cls),"mirror component creation failed");collision_off(component,r);return component;
}
void finish_component(Obj actor,Obj component,const Transform& relative,HsmpViewResult* r) {
    Function f(L"/Script/Engine.Actor:FinishAddComponent");f.object(L"Component",component);f.boolean(L"bManualAttachment",true);
    f.put(L"RelativeTransform",L"StructProperty",engine(relative),L"Transform");f.call(actor,r);collision_off(component,r);
}
struct Part {uint32_t id{},kind{};Obj render{},leader{};std::vector<Obj> materials;Obj native_asset{};std::wstring arm_socket;HsmpViewSpringArmFrame arm{};};
struct Mirror {Obj world{},actor{};std::vector<Part> parts;};
std::map<uint64_t,Mirror> mirrors;
std::mutex mirror_mutex;
uint64_t next_mirror{1};
void destroy_actor(Obj world,Obj actor) {
    require(same(actor_world(actor),world),"mirror destroy world mismatch");Function f(L"/Script/Engine.Actor:K2_DestroyActor");
    // Destroy invalidates the object, so only qualify immediately before the call.
    require(vt->is_a(get(actor),get(f.cls))!=0,"mirror destroy class");
    void* object=get(actor);void* function=get(f.function);check_guard();
    vt->call(object,function,f.buf.data());
    // The actor may now be dead. Only the borrowed world scope is checked.
    check_guard();
}
void initialize_result(HsmpViewResult* r) {require(r!=nullptr,"native result missing");*r={};}
void failure(HsmpViewResult* r,const char* why) {if(r){r->complete=0;std::snprintf(r->reason,sizeof(r->reason),"%s",why);}}
int32_t inspect(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewResult* r) {
    try{initialize_result(r);thread();OperationScope scope(guard,world);supported(world,owner,component,r);r->complete=1;return 1;}
    catch(const std::exception& e){failure(r,e.what());return -1;}
}
int32_t capture(Obj world,Obj owner,Obj component,const HsmpViewComponent* c,HsmpViewFrame* out,const HsmpViewGuard* guard,HsmpViewResult* r) {
    try{initialize_result(r);thread();OperationScope scope(guard,world);require(c&&out,"native capture arguments");layouts();frame(*c,*out);SplineOperation spline_scope(c->kind==5?owner:Obj{},c->kind==5?component:Obj{});VertexOperation vertex_scope(c->vertex_state==0||c->kind==8?owner:Obj{},c->vertex_state==0||c->kind==8?component:Obj{},c->kind==8);SceneOperation scene_scope(c->kind==6||c->kind==7?owner:Obj{},c->kind==6||c->kind==7?component:Obj{},c->kind);supported(world,owner,component,r);
        if(c->kind==0) {require(same(mesh_asset(component,r),asset(c->asset,L"/Script/Engine.SkeletalMesh")),"source mesh recipe changed");}
        else if(c->kind>=4)require(same(keep(vt->class_of(get(component))),asset(c->asset,L"/Script/CoreUObject.Class")),"source scene anchor class changed");
        else require(same(object_property(component,L"StaticMesh"),asset(c->asset,L"/Script/Engine.StaticMesh")),"source static mesh recipe changed");
        if(c->kind!=5)source_static(component,*c,r);if(c->kind<=1)verify_colors(component,*c,r);
        vertex_native_asset(world,owner,component,*c,r);
        capture_values(component,*c,*out,r);qualify(world,owner,component,r);
        if(c->kind==5)spline_capture(world,owner,component,c->spline,*out->spline,r);
        if(c->kind==4){scene_anchor(component);source_static(component,*c,r);}
        if(c->kind==7)*out->spring_arm=arm_observe(world,owner,component,c->spring_arm_socket,r);
        vertex_native_asset(world,owner,component,*c,r);
        r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
uint64_t create(Obj world,const HsmpViewComponent* recipes,uint32_t count,const HsmpViewGuard* guard,HsmpViewResult* r) {
    const std::lock_guard lock(mirror_mutex);
    Obj actor{};
    try{initialize_result(r);thread();OperationScope scope(guard,world);layouts();get(world);require(is(world,L"/Script/Engine.World")&&pointers(recipes,count,64)&&count>0,"mirror create bounds");
        require(mirrors.size()<256,"mirror registry capacity");for(uint32_t i=0;i<count;++i)recipe(recipes[i]);const auto order=parent_order(recipes,count);
        Transform identity{{0,0,0},{0,0,0,1},{1,1,1}};auto library=find(L"/Script/Engine.Default__GameplayStatics");
        Function begin(L"/Script/Engine.GameplayStatics:BeginDeferredActorSpawnFromClass");begin.object(L"WorldContextObject",world);begin.object(L"ActorClass",find(L"/Script/Engine.Actor"),true);
        begin.put(L"SpawnTransform",L"StructProperty",engine(identity),L"Transform");begin.enumeration(L"CollisionHandlingOverride",1);begin.object(L"Owner",{});
        begin.enumeration(L"TransformScaleMethod",0);begin.call(library,r);actor=begin.returned();require(actor.weak,"mirror actor creation failed");
        Function finish(L"/Script/Engine.GameplayStatics:FinishSpawningActor");finish.object(L"Actor",actor);finish.put(L"SpawnTransform",L"StructProperty",engine(identity),L"Transform");finish.enumeration(L"TransformScaleMethod",0);finish.call(library,r);
        require(same(finish.returned(),actor)&&same(actor_world(actor,r),world),"mirror actor world changed");
        Function actor_collision(L"/Script/Engine.Actor:SetActorEnableCollision");actor_collision.boolean(L"bNewActorEnableCollision",false);actor_collision.call(actor,r);
        Function actor_tick(L"/Script/Engine.Actor:SetActorTickEnabled");actor_tick.boolean(L"bEnabled",false);actor_tick.call(actor,r);
        Mirror mirror{world,actor,{}};
        for(uint32_t i=0;i<count;++i) {
            const auto& c=recipes[i];Part part{c.id,c.kind,{},{},{}};
            require(std::none_of(mirror.parts.begin(),mirror.parts.end(),[&](const Part& p){return p.id==c.id;}),"duplicate mirror component id");
            if(c.kind==0) {
                auto mesh=asset(c.asset,L"/Script/Engine.SkeletalMesh");
                part.leader=add_component(actor,L"/Script/Engine.PoseableMeshComponent",c.relative,r);
                Function leader_mesh(L"/Script/Engine.SkinnedMeshComponent:SetSkinnedAssetAndUpdate");leader_mesh.object(L"NewMesh",mesh);leader_mesh.boolean(L"bReinitPose",true);leader_mesh.call(part.leader,r);
                finish_component(actor,part.leader,c.relative,r);visibility(part.leader,false,r);
                part.render=add_component(actor,L"/Script/Engine.SkeletalMeshComponent",c.relative,r);
                Function disable_pp(L"/Script/Engine.SkeletalMeshComponent:SetDisablePostProcessBlueprint");disable_pp.boolean(L"bInDisablePostProcess",true);disable_pp.call(part.render,r);
                Function anim(L"/Script/Engine.SkeletalMeshComponent:SetAnimClass");anim.object(L"NewClass",{},true);anim.call(part.render,r);
                Function set_mesh(L"/Script/Engine.SkeletalMeshComponent:SetSkeletalMeshAsset");set_mesh.object(L"NewMesh",mesh);set_mesh.call(part.render,r);
                Function allow_cloth(L"/Script/Engine.SkeletalMeshComponent:SetAllowClothActors");allow_cloth.boolean(L"bInAllow",false);allow_cloth.call(part.render,r);
                Function follow(L"/Script/Engine.SkinnedMeshComponent:SetLeaderPoseComponent");follow.object(L"NewLeaderBoneComponent",part.leader);follow.boolean(L"bForceUpdate",true);follow.boolean(L"bInFollowerShouldTickPose",false);follow.call(part.render,r);
                finish_component(actor,part.render,c.relative,r);require(!object_property(part.render,L"AnimScriptInstance").weak&&!object_property(part.render,L"PostProcessAnimInstance").weak,"mirror animation instance active");
                require(same(mesh_asset(part.render,r),mesh)&&same(mesh_asset(part.leader,r),mesh),"mirror mesh assignment failed");
                for(uint32_t b=0;b<c.hidden_count;++b)for(auto target:{part.leader,part.render}) {
                    Function hide(L"/Script/Engine.SkinnedMeshComponent:HideBoneByName");hide.put(L"BoneName",L"NameProperty",name(c.hidden_bones[b]));hide.enumeration(L"PhysBodyOption",0);hide.call(target,r);
                    Function check(L"/Script/Engine.SkinnedMeshComponent:IsBoneHiddenByName");check.put(L"BoneName",L"NameProperty",name(c.hidden_bones[b]));check.call(target,r);
                    auto p=check.field(L"ReturnValue",L"BoolProperty",1);require((check.buf[static_cast<size_t>(p.offset+p.bool_offset)]&p.bool_mask)!=0,"mirror hidden bone readback failed");
                }
            }else if(c.kind==4) {
                part.render=add_component(actor,L"/Script/Engine.SceneComponent",c.relative,r);
                finish_component(actor,part.render,c.relative,r);
            }else if(c.kind==5) {
                part.render=add_component(actor,L"/Script/Engine.SplineComponent",c.relative,r);
                finish_component(actor,part.render,c.relative,r);
                Function tick(L"/Script/Engine.ActorComponent:SetComponentTickEnabled");tick.boolean(L"bEnabled",false);tick.call(part.render,r);
            }else if(c.kind==6||c.kind==7){
                part.render=add_component(actor,c.kind==6?L"/Script/Engine.CameraComponent":L"/Script/Engine.SpringArmComponent",c.relative,r);
                SceneOperation operation(actor,part.render,c.kind);finish_component(actor,part.render,c.relative,r);scene_inert(part.render,r);
                if(c.kind==7){part.arm_socket=text(c.spring_arm_socket);part.arm=arm_observe(world,actor,part.render,c.spring_arm_socket,r);}
            }else if(c.kind==8){
                part.render=add_component(actor,L"/Script/Engine.StaticMeshComponent",c.relative,r);
                VertexOperation operation(actor,part.render,true);require(vertex_empty,"native empty mirror constructor asset present");
                Function mesh(L"/Script/Engine.StaticMeshComponent:SetStaticMesh");mesh.object(L"NewMesh",{});mesh.call(part.render,r);
                finish_component(actor,part.render,c.relative,r);scene_inert(part.render,r);vertex_native_asset(world,actor,part.render,c,r);
            }else {
                part.render=add_component(actor,L"/Script/Engine.StaticMeshComponent",c.relative,r);
                auto mesh=asset(c.asset,L"/Script/Engine.StaticMesh");Function set_mesh(L"/Script/Engine.StaticMeshComponent:SetStaticMesh");set_mesh.object(L"NewMesh",mesh);set_mesh.call(part.render,r);
                if(c.vertex_state==0){part.native_asset=mesh;VertexOperation vertex_scope(actor,part.render);finish_component(actor,part.render,c.relative,r);}
                else finish_component(actor,part.render,c.relative,r);
                require(same(object_property(part.render,L"StaticMesh"),mesh),"mirror static asset readback failed");
            }
            std::optional<VertexOperation> vertex_scope;
            if(c.vertex_state==0||c.kind==8)vertex_scope.emplace(actor,part.render,c.kind==8);
            std::optional<SceneOperation> scene_scope;if(c.kind==6||c.kind==7)scene_scope.emplace(actor,part.render,c.kind);
            for(uint32_t j=0;j<c.material_count;++j) {
                const auto& m=c.materials[j];Function mid(L"/Script/Engine.PrimitiveComponent:CreateDynamicMaterialInstance");mid.put(L"ElementIndex",L"IntProperty",static_cast<int32_t>(m.slot));mid.object(L"SourceMaterial",asset(m.base,L"/Script/Engine.MaterialInterface"));mid.put(L"OptionalName",L"NameProperty",uint64_t{});mid.call(part.render,r);
                auto instance=mid.returned();require(instance.weak&&is(instance,L"/Script/Engine.MaterialInstanceDynamic")&&same(material(part.render,m.slot,r),instance),"mirror material creation failed");part.materials.push_back(instance);
            }
            if(c.kind<=1)colors(part.render,c,r);visibility(part.render,c.visible!=0,r);qualify(world,actor,part.render,r);
            if(part.leader.weak)qualify(world,actor,part.leader,r);mirror.parts.push_back(std::move(part));
        }
        // Source roots have no parent; every other direct parent must be in the
        // complete dictionary. Scene anchors preserve their native sockets.
        for(const auto i:order) {
            const auto& c=recipes[i];if(c.parent==0)continue;
            auto it=std::find_if(mirror.parts.begin(),mirror.parts.end(),[&](const Part& p){return p.id==c.parent;});
            require(it!=mirror.parts.end(),"mirror attachment parent missing");
            require(c.parent!=c.id,"mirror self attachment");
            for(auto target:{mirror.parts[i].render,mirror.parts[i].leader})if(target.weak) {
                Function attach(L"/Script/Engine.SceneComponent:K2_AttachToComponent");attach.object(L"Parent",it->render);attach.put(L"SocketName",L"NameProperty",name(c.socket));
                attach.enumeration(L"LocationRule",0);attach.enumeration(L"RotationRule",0);attach.enumeration(L"ScaleRule",0);attach.boolean(L"bWeldSimulatedBodies",false);attach.call(target,r);
            }
        }
        std::vector<HsmpViewFinishTarget> targets;for(const auto& part:mirror.parts){if(part.native_asset.weak)targets.push_back({actor,part.render,part.native_asset});
            else if(part.kind>=6)targets.push_back({actor,part.render,{},part.kind,1,{u16(part.arm_socket.c_str()),static_cast<uint32_t>(part.arm_socket.size()),0},part.arm});}
        finish_scene_set(world,targets,r);
        require(next_mirror!=0,"mirror handle exhausted");const auto id=next_mirror++;mirrors.emplace(id,std::move(mirror));r->complete=1;return id;
    }catch(const std::exception& e){failure(r,e.what());if(actor.weak){try{OperationScope scope(guard,world);destroy_actor(world,actor);}catch(const std::exception&){}}return 0;}
}
void world_transform(Obj component,const Transform& t,HsmpViewResult* r) {
    Function f(L"/Script/Engine.SceneComponent:K2_SetWorldTransform");f.put(L"NewTransform",L"StructProperty",engine(t),L"Transform");f.boolean(L"bSweep",false);f.boolean(L"bTeleport",true);f.call(component,r);
}
bool close(const Transform& a,const Transform& b) {
    for(size_t i=0;i<3;++i) if(std::abs(a.p[i]-b.p[i])>0.001||std::abs(a.scale[i]-b.scale[i])>0.00001)return false;
    double direct{},opposite{};for(size_t i=0;i<4;++i){direct+=std::abs(a.q[i]-b.q[i]);opposite+=std::abs(a.q[i]+b.q[i]);}
    return std::min(direct,opposite)<=0.00001;
}
template<class T> void material_set(Obj mat,const HsmpViewParameter& p,const T& value,const wchar_t* path,const wchar_t* type,const wchar_t* sub,HsmpViewResult* r) {
    Function f(path);f.put(L"ParameterInfo",L"StructProperty",parameter(p),L"MaterialParameterInfo");f.put(L"Value",type,value,sub);f.call(mat,r);
}
int32_t apply(Obj world,uint64_t id,const HsmpViewComponent* recipes,const HsmpViewFrame* frames,uint32_t count,const HsmpViewGuard* guard,HsmpViewResult* r) {
    const std::lock_guard lock(mirror_mutex);
    try{initialize_result(r);thread();OperationScope scope(guard,world);get(world);auto it=mirrors.find(id);require(it!=mirrors.end(),"mirror handle missing");auto& mirror=it->second;
        require(same(mirror.world,world)&&same(actor_world(mirror.actor,r),world)&&count==mirror.parts.size()&&pointers(recipes,count,64)&&pointers(frames,count,64),"mirror apply scope");
        const auto order=parent_order(recipes,count);
        for(const auto i:order) {
            const auto& c=recipes[i];const auto& f=frames[i];auto& part=mirror.parts[i];frame(c,f);require(c.id==part.id&&c.kind==part.kind,"mirror recipe generation mismatch");
            SplineOperation spline_scope(c.kind==5?mirror.actor:Obj{},c.kind==5?part.render:Obj{});
            VertexOperation vertex_scope(c.vertex_state==0||c.kind==8?mirror.actor:Obj{},c.vertex_state==0||c.kind==8?part.render:Obj{},c.kind==8);
            SceneOperation scene_scope(c.kind==6||c.kind==7?mirror.actor:Obj{},c.kind==6||c.kind==7?part.render:Obj{},c.kind);
            vertex_native_asset(world,mirror.actor,part.render,c,r);
            qualify(world,mirror.actor,part.render,r);world_transform(part.render,f.world,r);if(part.leader.weak)world_transform(part.leader,f.world,r);
            if(c.kind==5)spline_apply(world,mirror.actor,part.render,c.spline,*f.spline,r);
            if(c.kind==7){
                scene_profile(part.render,7);arm_socket(c.spring_arm_socket);qualify(world,mirror.actor,part.render,r);check_guard();vertex_pure(mirror.actor);scene_pure_profile(part.render,7);
                const auto expected_socket=name(c.spring_arm_socket);check_guard();vertex_pure(mirror.actor);scene_pure_profile(part.render,7);require(scene_socket_read()==expected_socket,"mirror spring socket changed");
                auto* p=static_cast<uint8_t*>(vertex_pure(part.render));std::memcpy(p+0x2f0,f.spring_arm->translation,24);std::memcpy(p+0x310,f.spring_arm->rotation,32);
                require(close(arm_transform(*f.spring_arm),arm_getter(part.render,c.spring_arm_socket,r)),"mirror native spring socket readback");part.arm=*f.spring_arm;
            }
            for(uint32_t j=0;j<c.bone_count;++j) {
                Function b(L"/Script/Engine.PoseableMeshComponent:SetBoneTransformByName");b.put(L"BoneName",L"NameProperty",name(c.bones[j]));b.put(L"InTransform",L"StructProperty",engine(f.bones[j]),L"Transform");b.enumeration(L"BoneSpace",1);b.call(part.leader,r);
            }
            for(uint32_t j=0;j<c.morph_count;++j) {
                require(std::isfinite(f.morphs[j]),"mirror morph invalid");Function m(L"/Script/Engine.SkeletalMeshComponent:SetMorphTarget");m.put(L"MorphTargetName",L"NameProperty",name(c.morphs[j]));m.put(L"Value",L"FloatProperty",f.morphs[j]);m.boolean(L"bRemoveZeroWeight",false);m.call(part.render,r);
            }
            uint32_t si{},vi{},ti{};
            for(uint32_t j=0;j<c.material_count;++j) {
                const auto& m=c.materials[j];auto mat=part.materials[j];require(same(material(part.render,m.slot,r),mat),"mirror material replaced");
                for(uint32_t k=0;k<m.scalar_count;++k) {float value=f.scalars[si++];require(std::isfinite(value),"mirror material scalar invalid");material_set(mat,m.scalars[k],value,L"/Script/Engine.MaterialInstanceDynamic:SetScalarParameterValueByInfo",L"FloatProperty",nullptr,r);
                    require(material_get<float>(mat,m.scalars[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetScalarParameterValueByInfo",L"FloatProperty",nullptr,r)==value,"mirror scalar readback failed");}
                for(uint32_t k=0;k<m.vector_count;++k) {std::array<float,4> value{};std::copy_n(f.vectors+4*vi++,4,value.data());for(float x:value)require(std::isfinite(x),"mirror material vector invalid");material_set(mat,m.vectors[k],value,L"/Script/Engine.MaterialInstanceDynamic:SetVectorParameterValueByInfo",L"StructProperty",L"LinearColor",r);
                    require(material_get<std::array<float,4>>(mat,m.vectors[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetVectorParameterValueByInfo",L"StructProperty",L"LinearColor",r)==value,"mirror vector readback failed");}
                for(uint32_t k=0;k<m.texture_count;++k) {auto texture=f.textures[ti++];void* ptr=texture.weak?get(texture):nullptr;material_set(mat,m.textures[k],ptr,L"/Script/Engine.MaterialInstanceDynamic:SetTextureParameterValueByInfo",L"ObjectProperty",nullptr,r);
                    require(material_get<void*>(mat,m.textures[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetTextureParameterValueByInfo",L"ObjectProperty",nullptr,r)==ptr,"mirror texture readback failed");}
            }
            require(close(transform(part.render,L"/Script/Engine.SceneComponent:K2_GetComponentToWorld",r),f.world),"mirror world transform readback failed");
            for(uint32_t j=0;j<c.bone_count;++j) {
                require(close(bone(part.leader,c.bones[j],r),f.bones[j]),"mirror leader transform readback failed");
                require(close(bone(part.render,c.bones[j],r),f.bones[j]),"mirror rendered bone transform readback failed");
            }
            for(uint32_t j=0;j<c.morph_count;++j){Function m(L"/Script/Engine.SkeletalMeshComponent:GetMorphTarget");m.put(L"MorphTargetName",L"NameProperty",name(c.morphs[j]));m.call(part.render,r);require(m.value<float>(L"ReturnValue",L"FloatProperty")==f.morphs[j],"mirror morph readback failed");}
            qualify(world,mirror.actor,part.render,r);if(part.leader.weak)qualify(world,mirror.actor,part.leader,r);
            vertex_native_asset(world,mirror.actor,part.render,c,r);
        }
        std::vector<HsmpViewFinishTarget> targets;for(const auto& part:mirror.parts){if(part.native_asset.weak)targets.push_back({mirror.actor,part.render,part.native_asset});
            else if(part.kind>=6)targets.push_back({mirror.actor,part.render,{},part.kind,1,{u16(part.arm_socket.c_str()),static_cast<uint32_t>(part.arm_socket.size()),0},part.arm});}
        finish_scene_set(world,targets,r);r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
void destroy(Obj world,uint64_t id,const HsmpViewGuard* guard) {const std::lock_guard lock(mirror_mutex);try{thread();auto it=mirrors.find(id);if(it==mirrors.end())return;auto mirror=it->second;mirrors.erase(it);require(same(world,mirror.world),"mirror destroy scope");OperationScope scope(guard,world);destroy_actor(world,mirror.actor);}catch(const std::exception&) {}}
void discard(uint64_t id) {const std::lock_guard lock(mirror_mutex);mirrors.erase(id);}
using ObjectFlags=const uint32_t*(*)(const void*);
ObjectFlags retirement_flags{};
Free retirement_free{};
using RetirementIndex=void*(*)(int32_t);
using RetirementSlotObject=void**(*)(void*);
using RetirementSlotSerial=int32_t*(*)(void*);
RetirementIndex retirement_index{};RetirementSlotObject retirement_object{};RetirementSlotSerial retirement_serial{};
using SourceOuter=const void* const*(*)(const void*);
SourceOuter source_outer{};
const uint64_t* source_package_name{};
// This is called from the borrowed provider guard itself. Never use get/keep,
// check_guard, Function or string conversion here: those would reenter it.
void* source_path_get(const HsmpNativePathNode& node) {
    require(vt&&object_name&&retirement_flags&&source_outer,"native path metadata unavailable");
    void* object=vt->resolve(node.weak);require(node.weak&&object&&reinterpret_cast<uint64_t>(object)==node.address,"native path original slot/address changed");
    auto flags=retirement_flags(object);require(flags&&(*flags&0x40000000u)==0,"native path original object garbage");
    void* cls=vt->resolve(node.class_weak);require(node.class_weak&&cls&&reinterpret_cast<uint64_t>(cls)==node.class_address,"native path original class slot changed");
    flags=retirement_flags(cls);require(flags&&(*flags&0x40000000u)==0,"native path original class garbage");
    auto object_name_value=object_name(object);auto class_name_value=object_name(cls);
    require(object_name_value&&*object_name_value==node.name&&class_name_value&&*class_name_value==node.class_name&&vt->class_of(object)==cls,"native path original FName/class changed");return object;
}
HsmpNativePathNode source_path_node(void* object) {
    require(object!=nullptr,"native path node missing");HsmpNativePathNode node{};node.weak=vt->weak(object);node.address=reinterpret_cast<uint64_t>(object);
    require(node.weak&&vt->resolve(node.weak)==object,"native path node weak unavailable");auto flags=retirement_flags(object);require(flags&&(*flags&0x40000000u)==0,"native path node garbage");
    void* cls=vt->class_of(object);require(cls!=nullptr,"native path class missing");node.class_weak=vt->weak(cls);node.class_address=reinterpret_cast<uint64_t>(cls);
    require(node.class_weak&&vt->resolve(node.class_weak)==cls,"native path class weak unavailable");flags=retirement_flags(cls);require(flags&&(*flags&0x40000000u)==0,"native path class garbage");
    const auto own=object_name(object),class_name_value=object_name(cls);require(own&&class_name_value,"native path FName unavailable");node.name=*own;node.class_name=*class_name_value;source_path_get(node);return node;
}
void source_path_verify(const HsmpNativePathNode* nodes,uint32_t count,uint64_t package_name) {
    require(source_package_name&&*source_package_name==package_name,"native path package discriminator changed");
    require(nodes&&count>0&&count<=64,"native path hierarchy bounds");
    for(uint32_t i=0;i<count;++i){for(uint32_t j=0;j<i;++j)require(nodes[i].address!=nodes[j].address,"native path hierarchy cycle");
        void* object=source_path_get(nodes[i]);const void* const* field=source_outer(object);require(field!=nullptr,"native path Outer field unavailable");
        const void* outer{};std::memcpy(&outer,field,sizeof(outer));require(reinterpret_cast<uint64_t>(outer)==(i+1<count?nodes[i+1].address:0),"native path original Outer link changed");source_path_get(nodes[i]);}
    require(*source_package_name==package_name,"native path package discriminator changed during walk");
}
int32_t source_path_reader(const HsmpNativePathNode* nodes,uint32_t count,uint32_t capture,HsmpNativePathNode* output,uint32_t capacity,uint32_t* output_count,uint64_t* package_name,char* reason,uint32_t reason_capacity) {
    try{require(vt&&vt->abi==HSMP_REFLECT_ABI,"native path reflection unavailable");const DWORD current=GetCurrentThreadId();if(!game_thread)game_thread=current;require(current==game_thread,"native path game-thread admission");
        require(object_name&&retirement_flags&&source_outer&&source_package_name&&package_name,"native path provider unavailable");
        if(capture){require(capture==1&&nodes&&count==1&&output&&capacity==64&&output_count,"native path capture bounds");*output_count=0;
            const auto package=*source_package_name;auto node=nodes[0];uint32_t length{};
            while(true){require(length<64,"native path hierarchy exceeds64");for(uint32_t i=0;i<length;++i)require(output[i].address!=node.address,"native path hierarchy cycle");output[length++]=node;
                void* object=source_path_get(node);const auto* field=source_outer(object);require(field!=nullptr,"native path Outer field unavailable");const void* outer{};std::memcpy(&outer,field,sizeof(outer));source_path_get(node);if(!outer)break;node=source_path_node(const_cast<void*>(outer));}
            source_path_verify(output,length,package);*package_name=package;*output_count=length;
        }else source_path_verify(nodes,count,*package_name);
        return 1;
    }catch(const std::exception& e){if(reason&&reason_capacity)std::snprintf(reason,reason_capacity,"%s",e.what());return -1;}
}
constexpr uint32_t mirrored_garbage=0x40000000; // matched shipping actor iterator/Kismet:IsValid, evidence-20261005
struct RetirementLayout {HsmpProp root{},destroying{},role{},remote{};uint32_t known{};};
struct RetiredDriver {Obj world{},actor{};Identity identity{};HsmpViewLifecycle before{};RetirementLayout layout{};};
std::map<uint64_t,RetiredDriver> retired_drivers;
void forget_retirements(){retired_drivers.clear();}
void* original_slot(Obj actor,const Identity& original) {
    check_guard();require(retirement_index&&retirement_object&&retirement_serial,"native object-array slot API unavailable");
    void* item=retirement_index(static_cast<int32_t>(actor.weak));check_guard();if(!item)return nullptr;
    const auto object=retirement_object(item);const auto serial=retirement_serial(item);check_guard();
    require(object&&serial,"native object-array slot metadata unavailable");if(!*object)return nullptr;
    require(reinterpret_cast<uint64_t>(*object)==actor.address&&(!(actor.weak>>32)||static_cast<uint32_t>(*serial)==static_cast<uint32_t>(actor.weak>>32)),"native retired object-array identity reused");
    void* p=vt->resolve(actor.weak);check_guard();require(p!=nullptr,"native original UObject memory qualification unavailable");
    require(reinterpret_cast<uint64_t>(p)==actor.address,"native retired weak identity reused");
    void* cls=vt->resolve(original.class_weak);check_guard();const auto n=object_name(p);check_guard();
    require(cls&&reinterpret_cast<uint64_t>(cls)==original.class_address&&vt->class_of(p)==cls&&n&&*n==original.name,
        "native retired name/class identity reused");check_guard();return p;
}
uint32_t flags_of(void* object){check_guard();require(retirement_flags!=nullptr,"native object flags API unavailable");const auto p=retirement_flags(object);check_guard();require(p!=nullptr,"native object flags unavailable");return *p;}
bool world_contains(Obj world,const Identity& original,uint64_t address){
    require(retirement_free!=nullptr,"native actor-array allocator unavailable");
    const Obj cls{original.class_weak,original.class_address};get(cls);
    Function f(L"/Script/Engine.GameplayStatics:GetAllActorsOfClass");
    f.object(L"WorldContextObject",world);f.object(L"ActorClass",cls,true);
    const auto field=f.field(L"OutActors",L"ArrayProperty",16);OwnedColorArray owned{retirement_free,f.buf.data()+field.offset};
    f.call(find(L"/Script/Engine.Default__GameplayStatics"));
    const auto a=f.value<Array>(L"OutActors",L"ArrayProperty");
    require(a.count>=0&&a.count<=512&&a.capacity>=a.count&&(!a.count||a.data),"native driver world census bounds");
    bool listed{};const auto* rows=static_cast<void*const*>(a.data);
    for(int32_t i=0;i<a.count;++i)if(reinterpret_cast<uint64_t>(rows[i])==address)listed=true;
    check_guard();return listed;
}
RetirementLayout retirement_layout(Obj actor){
    RetirementLayout out;
    const auto read_field=[&](HsmpProp& field,const wchar_t* key,const wchar_t* type,int size,uint32_t bit){
        try{field=property(actor,key,type,size);out.known|=bit;}catch(const Error&){check_guard();}
    };
    read_field(out.root,L"RootComponent",L"ObjectProperty",8,16);
    read_field(out.destroying,L"bActorIsBeingDestroyed",L"BoolProperty",1,2);
    if((out.known&2)&&(!out.destroying.bool_mask||out.destroying.bool_offset!=0))out.known&=~2u;
    read_field(out.role,L"Role",L"ByteProperty",1,8);read_field(out.remote,L"RemoteRole",L"ByteProperty",1,128);
    if(!(out.known&128))out.known&=~8u;out.known&=~128u;return out;
}
HsmpViewLifecycle lifecycle(void* p,const Identity& original,const RetirementLayout& layout){
    HsmpViewLifecycle out{};out.flags=flags_of(p);out.known=65;out.name=original.name;out.class_weak=original.class_weak;out.class_address=original.class_address;
    // Reflected field layouts were verified while the original actor was live.
    // These diagnostic reads make no actor ProcessEvent call after destruction.
    const auto* bytes=static_cast<const uint8_t*>(p);
    if(layout.known&2){out.destroying=(bytes[layout.destroying.offset]&layout.destroying.bool_mask)!=0;out.known|=2;}
    if(layout.known&8){out.local_role=bytes[layout.role.offset];out.remote_role=bytes[layout.remote.offset];out.authority=out.local_role==3;out.known|=8;}
    if(layout.known&16){void* root{};std::memcpy(&root,bytes+layout.root.offset,8);out.root_address=reinterpret_cast<uint64_t>(root);out.known|=16;
        if(root){out.root_weak=vt->weak(root);check_guard();if(out.root_weak&&vt->resolve(out.root_weak)==root)out.root_live=(flags_of(root)&mirrored_garbage)==0;}}
    check_guard();return out;
}
bool retirement_after(Obj world,Obj actor,const Identity& identity,const RetirementLayout& layout,HsmpViewRetirement* result){
    const bool listed=world_contains(world,identity,actor.address);
    void* p=original_slot(actor,identity);result->weak_present=p?1:0;
    if(p)result->after=lifecycle(p,identity,layout);
    result->after.listed=listed;result->after.known|=4;
    const bool retired=!result->after.listed&&(!p||(result->after.flags&mirrored_garbage)!=0);
    result->alive_after=retired?0:1;return retired;
}
int32_t retire(Obj world,Obj actor,uint32_t kind,Obj target,const HsmpViewGuard* guard,HsmpViewRetirement* result) {
    try {
        require(result!=nullptr,"native retirement result missing");*result={};result->alive_after=2;result->weak_present=2;
        thread();OperationScope scope(guard,world);
        require(kind==0&&!target.weak&&!target.address,"native retirement kind unsupported");
        require(retired_drivers.size()<512&&!retired_drivers.contains(actor.weak),"native retirement proof bound or reused identity");
        get(actor);result->weak=actor.weak;result->address=actor.address;
        require(is(actor,L"/Script/Engine.Actor"),"native retirement actor class");
        bool driver{};
        for(const auto* path:{L"/Game/Blueprints/Managers/BP_LevelManager.BP_LevelManager_C",
            L"/Game/Blueprints/Spawner/BP_SpawnerPoint_Willies.BP_SpawnerPoint_Willies_C",
            L"/Game/Blueprints/Generators/BP_Generator_Weapons_Random.BP_Generator_Weapons_Random_C"}) {
            check_guard();void* class_object=vt->find(u16(path));check_guard();
            if(class_object&&vt->class_of(get(actor))==get(keep(class_object)))driver=true;
        }
        require(driver,"native retirement driver class unsupported");
        const auto identity=identities.at(actor.weak);const auto layout=retirement_layout(actor);
        result->before=lifecycle(get(actor),identity,layout);
        require((result->before.flags&mirrored_garbage)==0,"native retirement original actor is garbage");
        result->before.listed=world_contains(world,identity,actor.address);result->before.known|=4;
        require(result->before.listed!=0,"native retirement original actor not in world census");
        const auto original_level=returned(actor,L"/Script/Engine.Actor:GetLevel");
        result->before.level_weak=original_level.weak;result->before.level_address=original_level.address;result->before.known|=32;
        require(same(actor_world(actor),world),"native retirement actor world mismatch");
        Function tag(L"/Script/Engine.Actor:ActorHasTag");tag.put(L"Tag",L"NameProperty",name(L"Persistent"));tag.call(actor);
        const auto tag_property=tag.field(L"ReturnValue",L"BoolProperty",1);
        require(tag_property.bool_mask!=0&&tag_property.bool_offset==0,"native retirement tag layout");
        require((tag.buf[static_cast<size_t>(tag_property.offset+tag_property.bool_offset)]&tag_property.bool_mask)==0,
            "native retirement Persistent actor forbidden");
        const auto owner=returned(actor,L"/Script/Engine.Actor:GetOwner");
        require(!owner.weak||(!same(owner,actor)&&same(actor_world(owner),world)),"native retirement owner world mismatch");
        Function f(L"/Script/Engine.Actor:K2_DestroyActor");require(f.fields.empty(),"native retirement destroy signature changed");
        require(vt->is_a(get(actor),get(f.cls))!=0,"native retirement destroy owner class");
        void* object=get(actor);void* function=get(f.function);check_guard();require((flags_of(object)&mirrored_garbage)==0,"native actor changed before retirement dispatch");result->qualified=1;
        result->dispatched=1;vt->call(object,function,f.buf.data());
        // Never invoke actor GetWorld/GetLevel/ProcessEvent after dispatch.
        check_guard();require(retirement_after(world,actor,identity,layout,result),"native actor retirement not proved");
        retired_drivers.emplace(actor.weak,RetiredDriver{world,actor,identity,result->before,layout});return 1;
    }catch(const std::exception& e){if(result)std::snprintf(result->reason,sizeof(result->reason),"%s",e.what());return -1;}
}
int32_t probe_retirement(Obj world,Obj actor,const HsmpViewGuard* guard,HsmpViewRetirement* result){
    try {
        require(result!=nullptr,"native retirement probe result missing");*result={};result->alive_after=2;result->weak_present=2;
        thread();OperationScope scope(guard,world);
        const auto record=retired_drivers.find(actor.weak);
        require(record!=retired_drivers.end()&&same(record->second.world,world)&&same(record->second.actor,actor),"native original retirement proof unavailable");
        result->qualified=1;result->weak=actor.weak;result->address=actor.address;
        result->before=record->second.before;
        require(retirement_after(world,actor,record->second.identity,record->second.layout,result),"native original actor retirement no longer proved");return 1;
    }catch(const std::exception& e){if(result)std::snprintf(result->reason,sizeof(result->reason),"%s",e.what());return -1;}
}
int32_t actor_scope(Obj world,Obj actor,const HsmpViewGuard* guard,HsmpViewActorScope* result){
    try {
        require(result!=nullptr,"native actor scope result missing");*result={};
        thread();OperationScope scope(guard,world);get(actor);
        require(is(actor,L"/Script/Engine.Actor"),"native actor scope class");
        result->weak=actor.weak;result->address=actor.address;
        const auto identity=identities.at(actor.weak);const auto layout=retirement_layout(actor);
        const bool listed=world_contains(world,identity,actor.address);
        // The static census may reenter. Never accept metadata captured before
        // it, nor resolve a reused/disappeared original as a fresh actor.
        void* original=original_slot(actor,identity);require(original!=nullptr,"native actor scope original disappeared");
        result->state=lifecycle(original,identity,layout);result->state.listed=listed;result->state.known|=4;
        result->qualified=1;return 1;
    }catch(const std::exception& e){if(result)std::snprintf(result->reason,sizeof(result->reason),"%s",e.what());return -1;}
}
int32_t describe_spline(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewSplineProfile* out,HsmpViewResult* r) {
    const StaticProfileTraceScope tracing;
    try{profile_phase("cpp_admission",0);initialize_result(r);thread();OperationScope scope(guard,world);require(out!=nullptr,"native spline profile output missing");profile_phase("cpp_admission",1);
        profile_phase("core_layout",0);layouts();profile_phase("core_layout",1);
        auto snapshot=spline_coherent(world,owner,component,r);*out=spline_profile(snapshot);spline_profile_valid(*out);r->complete=1;return 1;}
    catch(const std::exception& e){failure(r,e.what());return -1;}
}
int32_t describe_vertex_state(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewVertexState* out,HsmpViewResult* r){
    try{initialize_result(r);thread();OperationScope scope(guard,world);require(out!=nullptr,"native vertex state output missing");
        *out=vertex_observe(world,owner,component,r);r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
int32_t finish_scene_sets(Obj world,const HsmpViewFinishTarget* source,uint32_t source_count,const uint64_t* handles,uint32_t mirror_count,const HsmpViewGuard* guard,HsmpViewResult* r){
    const std::lock_guard lock(mirror_mutex);
    try{initialize_result(r);thread();OperationScope scope(guard,world);
        require(pointers(source,source_count,32*64)&&pointers(handles,mirror_count,32)&&(!source_count||!mirror_count),"native vertex complete set arguments");
        std::vector<HsmpViewFinishTarget> targets;if(source_count)targets.assign(source,source+source_count);
        for(uint32_t i=0;i<mirror_count;++i){
            require(std::find(handles,handles+i,handles[i])==handles+i,"native vertex duplicate mirror handle");
            const auto found=mirrors.find(handles[i]);require(found!=mirrors.end()&&same(found->second.world,world),"native vertex mirror generation");
            for(const auto& part:found->second.parts){if(part.native_asset.weak)targets.push_back({found->second.actor,part.render,part.native_asset});
                else if(part.kind>=6)targets.push_back({found->second.actor,part.render,{},part.kind,1,{u16(part.arm_socket.c_str()),static_cast<uint32_t>(part.arm_socket.size()),0},part.arm});}
        }
        finish_scene_set(world,targets,r);r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
const HsmpPresentation provider{10,0,inspect,capture,create,apply,destroy,discard,retire,probe_retirement,forget_retirements,actor_scope,describe_spline,describe_vertex_state,finish_scene_sets};
}
void hsmp_presentation_register(const HsmpReflect* reflection) {
    const auto module=GetModuleHandleW(L"UE4SS.dll");
    object_name=reinterpret_cast<NamePrivate>(module?GetProcAddress(module,"?GetNamePrivate@UObjectBase@Unreal@RC@@QEBAAEBVFName@23@XZ"):nullptr);
    object_world=reinterpret_cast<GetWorld>(module?GetProcAddress(module,"?GetWorld@UObject@Unreal@RC@@QEBAPEAVUWorld@23@XZ"):nullptr);
    retirement_flags=reinterpret_cast<ObjectFlags>(module?GetProcAddress(module,"?GetObjectFlags@UObjectBase@Unreal@RC@@QEBAAEBW4EObjectFlags@23@XZ"):nullptr);
    spline_flags=retirement_flags;
    vertex_flags=retirement_flags;
    retirement_free=reinterpret_cast<Free>(module?GetProcAddress(module,"?Free@FMemory@Unreal@RC@@SAXPEAX@Z"):nullptr);
    retirement_index=reinterpret_cast<RetirementIndex>(module?GetProcAddress(module,"?IndexToObject@FUObjectArray@Unreal@RC@@SAPEAUFUObjectItem@23@H@Z"):nullptr);
    retirement_object=reinterpret_cast<RetirementSlotObject>(module?GetProcAddress(module,"?GetObject@FUObjectItem@Unreal@RC@@AEAAAEAPEAVUObjectBase@23@XZ"):nullptr);
    retirement_serial=reinterpret_cast<RetirementSlotSerial>(module?GetProcAddress(module,"?GetSerialNumber@FUObjectItem@Unreal@RC@@QEAAAEAHXZ"):nullptr);
    source_outer=reinterpret_cast<SourceOuter>(module?GetProcAddress(module,"?GetOuterPrivate@UObjectBase@Unreal@RC@@QEBAAEAPEBVUObject@23@XZ"):nullptr);
    source_package_name=reinterpret_cast<const uint64_t*>(module?GetProcAddress(module,"?GPackageName@Unreal@RC@@3VFName@12@A"):nullptr);
    spline_api.allocate=reinterpret_cast<SplineMalloc>(module?GetProcAddress(module,"?Malloc@FMemory@Unreal@RC@@SAPEAX_KI@Z"):nullptr);
    spline_api.release=retirement_free;
    spline_api.children=reinterpret_cast<SplineChildren>(module?GetProcAddress(module,"?GetChildProperties@UStruct@Unreal@RC@@QEAAAEAPEAVFField@23@XZ"):nullptr);
    spline_api.next=reinterpret_cast<SplineNext>(module?GetProcAddress(module,"?GetNextFieldAsProperty@FField@Unreal@RC@@QEAAPEAVFProperty@23@XZ"):nullptr);
    spline_api.inner=reinterpret_cast<SplineInner>(module?GetProcAddress(module,"?GetInner@FArrayProperty@Unreal@RC@@QEAAAEAPEAVFProperty@23@XZ"):nullptr);
    spline_api.structure=reinterpret_cast<SplineStruct>(module?GetProcAddress(module,"?GetStruct@FStructProperty@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUScriptStruct@Unreal@RC@@@23@XZ"):nullptr);
    spline_api.size=reinterpret_cast<SplineInt>(module?GetProcAddress(module,"?GetElementSize@FProperty@Unreal@RC@@QEAAAEAHXZ"):nullptr);
    spline_api.offset=reinterpret_cast<SplineInt>(module?GetProcAddress(module,"?GetOffset_Internal@FProperty@Unreal@RC@@QEAAAEAHXZ"):nullptr);
    spline_api.field_name=reinterpret_cast<SplineFieldName>(module?GetProcAddress(module,"?GetFName@FField@Unreal@RC@@QEBA?AVFName@23@XZ"):nullptr);
    spline_api.field_class=reinterpret_cast<SplineFieldClass>(module?GetProcAddress(module,"?GetClass@FField@Unreal@RC@@QEAA?AVFFieldClassVariant@23@XZ"):nullptr);
    spline_api.variant_name=reinterpret_cast<SplineVariantName>(module?GetProcAddress(module,"?GetFName@FFieldClassVariant@Unreal@RC@@QEBA?AVFName@23@XZ"):nullptr);
    vt=reflection;game_thread=0;names.clear();signatures.clear();identities.clear();mirrors.clear();retired_drivers.clear();
    layouts_verified=false;layout_objects.clear();spline_layout_verified=false;spline_layout_objects.clear();hsmp_native_set_presentation(vt?&provider:nullptr);
    hsmp_native_set_source_path_reader(vt&&source_outer&&source_package_name&&object_name&&retirement_flags?source_path_reader:nullptr);
}
