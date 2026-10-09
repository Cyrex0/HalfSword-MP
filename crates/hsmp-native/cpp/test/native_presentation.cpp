// Rejection/lifetime checks for the production provider; no mocked native parity claim.
#include "../src/native_presentation.cpp"
#include <iostream>
#include <limits>
#include <thread>
#include <cstdlib>
static const HsmpPresentation* installed{};
extern "C" void hsmp_native_set_presentation(const HsmpPresentation* p) {installed=p;}
// Production profile tracing is Rust-owned and inactive for this engine fixture.
int profile_ffi_calls{};
extern "C" void hsmp_native_profile_checkpoint(const char*,uint32_t){++profile_ffi_calls;}
extern "C" void hsmp_native_profile_tick(uint32_t){++profile_ffi_calls;}
extern "C" void hsmp_native_set_source_path_reader(HsmpNativePathReader){}
namespace {
int checks{}, touches{};
int spline_allocations{},spline_releases{},spline_fail_after{-1};
void* test_spline_allocate(uint64_t bytes,uint32_t alignment){++spline_allocations;if(spline_fail_after>=0&&spline_allocations>spline_fail_after)return nullptr;return _aligned_malloc(static_cast<size_t>(bytes),alignment);}
void test_spline_release(void* p){if(p){++spline_releases;_aligned_free(p);}}
void check(bool ok,const char* why) {++checks;if(!ok)throw std::runtime_error(why);}
template<class F> void rejects(F f,const char* why) {bool refused{};try{f();}catch(const Error&){refused=true;}check(refused,why);}
struct MockObject {uint64_t name;MockObject* cls;};
MockObject cls{10,nullptr}, obj{11,&cls};
uint64_t weak(void* p) {return p==&cls?2:p==&obj?1:0;}
void* resolve(uint64_t id) {++touches;return id==2?&cls:id==1?&obj:nullptr;}
void* class_of(void* p) {++touches;return p==&cls?&cls:static_cast<MockObject*>(p)->cls;}
const uint64_t* mock_name(const void* p) {++touches;return &static_cast<const MockObject*>(p)->name;}
int32_t guard_check(void* context) {return *static_cast<int32_t*>(context);}
// A small reflected Actor:GetLevel/K2_DestroyActor path exercises the production
// destroy entry, including a world change inside ProcessEvent while Lua's world
// token still reports valid. These are lifetime tests, not rendering proof.
struct LifetimeObject {uint64_t name{};LifetimeObject* cls{};LifetimeObject* property{};bool alive{true};uint32_t flags{};uint8_t destroying{},role{3},remote{};LifetimeObject* outer{};};
LifetimeObject meta{100},world_class{101},gi_class{102},actor_class{103},level_class{104},function_class{105};
LifetimeObject old_world{110,&world_class},new_world{111,&world_class},gi{112,&gi_class};
LifetimeObject actor{113,&actor_class},level{114,&level_class};
LifetimeObject get_level_fn{115,&function_class},destroy_fn{116,&function_class};
LifetimeObject driver_class{117,&meta},tag_fn{118,&function_class},owner_fn{119,&function_class},replacement{120,&driver_class};
LifetimeObject foreign_owner{121,&actor_class},foreign_level{122,&level_class};
LifetimeObject statics_class{123,&meta},statics{124,&statics_class},census_fn{125,&function_class};
LifetimeObject path_component{126,&actor_class},path_package{127,&meta};
uint64_t path_package_name{700};
bool path_mutate_name{},path_mutate_class{},path_mutate_package{};
const void* const* lifetime_outer(const void* p){auto o=const_cast<LifetimeObject*>(static_cast<const LifetimeObject*>(p));
    if(path_mutate_name)o->name^=1;if(path_mutate_class)o->cls->name^=1;if(path_mutate_package)path_package_name^=1;
    return reinterpret_cast<const void* const*>(&o->outer);}
LifetimeObject* retirement_owner{};
std::vector<LifetimeObject*> lifetime_objects;
std::map<std::wstring,uint64_t> lifetime_names;
LifetimeObject* current_world{};
bool travel_on_get_level{};
bool actor_persistent{},destroy_invalidates{true},reuse_after_destroy{},travel_on_destroy{};
bool destroy_garbage{},omit_world{},omit_after_destroy{},reuse_during_post_census{},travel_during_post_census{};
bool actor_memory_unqualified{};
bool scope_reuse_during_census{},scope_travel_during_census{},scope_garbage_during_census{},scope_disappear_during_census{};
bool spline_garbage_after_call{};
int post_destroy_actor_events{},array_frees{};
int level_calls{},destroy_calls{},post_destroy_actor_touches{},invalid_actor_resolves{};
void lifetime_touch(LifetimeObject* o) {if(o==&actor&&!actor.alive)++post_destroy_actor_touches;}
uint64_t lifetime_weak(void* p) {for(size_t i=0;i<lifetime_objects.size();++i)if(lifetime_objects[i]==p)return i+1;return 0;}
void* lifetime_resolve(uint64_t id) {
    if(id==0||id>lifetime_objects.size())return nullptr;
    auto o=lifetime_objects[static_cast<size_t>(id-1)];
    if(o==&actor&&destroy_calls&&reuse_after_destroy)return &replacement;
    if(o==&actor&&actor_memory_unqualified)return nullptr;
    if(o==&actor&&!actor.alive)++invalid_actor_resolves;return o->alive?o:nullptr;
}
void* lifetime_class(void* p) {auto o=static_cast<LifetimeObject*>(p);lifetime_touch(o);return o->cls;}
const uint64_t* lifetime_name(const void* p) {auto o=const_cast<LifetimeObject*>(static_cast<const LifetimeObject*>(p));lifetime_touch(o);return &o->name;}
const uint32_t* lifetime_flags(const void* p){auto o=static_cast<const LifetimeObject*>(p);return &o->flags;}
void lifetime_free(void* p){++array_frees;std::free(p);}
void* lifetime_index(int32_t index){return index>0&&static_cast<size_t>(index)<=lifetime_objects.size()?lifetime_objects[static_cast<size_t>(index-1)]:nullptr;}
void** lifetime_slot_object(void* item){static void* raw;auto o=static_cast<LifetimeObject*>(item);raw=o->alive?o:nullptr;if(o==&actor&&destroy_calls&&reuse_after_destroy)raw=&replacement;return &raw;}
int32_t* lifetime_slot_serial(void*){static int32_t serial=0;return &serial;}
uint64_t lifetime_fname(const uint16_t* key,int32_t) {
    auto text=std::wstring(reinterpret_cast<const wchar_t*>(key));auto [it,_]=lifetime_names.emplace(text,lifetime_names.size()+1);return it->second;
}
void* lifetime_find(const uint16_t* key) {
    const std::wstring path(reinterpret_cast<const wchar_t*>(key));
    if(path==L"/Script/Engine.World")return &world_class;
    if(path==L"/Script/Engine.GameInstance")return &gi_class;
    if(path==L"/Script/Engine.Actor")return &actor_class;
    if(path==L"/Script/Engine.Level")return &level_class;
    if(path==L"/Script/Engine.Actor:GetLevel")return &get_level_fn;
    if(path==L"/Script/Engine.Actor:K2_DestroyActor")return &destroy_fn;
    if(path==L"/Script/Engine.Actor:ActorHasTag")return &tag_fn;
    if(path==L"/Script/Engine.Actor:GetOwner")return &owner_fn;
    if(path==L"/Script/Engine.GameplayStatics")return &statics_class;
    if(path==L"/Script/Engine.Default__GameplayStatics")return &statics;
    if(path==L"/Script/Engine.GameplayStatics:GetAllActorsOfClass")return &census_fn;
    if(path==L"/Game/Blueprints/Managers/BP_LevelManager.BP_LevelManager_C")return &driver_class;
    return nullptr;
}
int32_t lifetime_is_a(void* object,void* type) {auto o=static_cast<LifetimeObject*>(object);lifetime_touch(o);return o->cls==type||(o->cls==&driver_class&&type==&actor_class);}
HsmpProp object_field(const wchar_t* key,int32_t offset) {
    HsmpProp p{};p.name=lifetime_fname(u16(key),1);p.cls=lifetime_fname(u16(L"ObjectProperty"),1);p.offset=offset;p.size=8;return p;
}
int32_t lifetime_props(void* fn,HsmpProp* out,int32_t cap,int32_t* size) {
    if(fn==&destroy_fn){*size=0;return 0;}
    if((fn==&get_level_fn||fn==&owner_fn)&&cap>0){*size=8;out[0]=object_field(L"ReturnValue",0);return 1;}
    if(fn==&tag_fn&&cap>=2){
        *size=16;out[0]=object_field(L"Tag",0);out[0].cls=lifetime_fname(u16(L"NameProperty"),1);
        out[1]=object_field(L"ReturnValue",8);out[1].cls=lifetime_fname(u16(L"BoolProperty"),1);out[1].size=1;out[1].bool_mask=1;return 2;
    }
    if(fn==&census_fn&&cap>=3){
        *size=32;out[0]=object_field(L"WorldContextObject",0);out[1]=object_field(L"ActorClass",8);out[1].cls=lifetime_fname(u16(L"ClassProperty"),1);
        out[2]=object_field(L"OutActors",16);out[2].cls=lifetime_fname(u16(L"ArrayProperty"),1);out[2].size=16;return 3;
    }
    return -1;
}
int32_t lifetime_prop(void* object,const uint16_t* key,HsmpProp* out) {
    const std::wstring field(reinterpret_cast<const wchar_t*>(key));
    if((object==&old_world||object==&new_world)&&field==L"OwningGameInstance") {*out=object_field(L"OwningGameInstance",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    if(object==&level&&field==L"OwningWorld") {*out=object_field(L"OwningWorld",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    if(object==&actor&&field==L"RootComponent"){*out=object_field(L"RootComponent",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    if(object==&actor&&(field==L"Role"||field==L"RemoteRole")){*out=object_field(field.c_str(),static_cast<int32_t>(field==L"Role"?offsetof(LifetimeObject,role):offsetof(LifetimeObject,remote)));out->cls=lifetime_fname(u16(L"ByteProperty"),1);out->size=1;return 1;}
    if(object==&actor&&field==L"bActorIsBeingDestroyed"){*out=object_field(field.c_str(),static_cast<int32_t>(offsetof(LifetimeObject,destroying)));out->cls=lifetime_fname(u16(L"BoolProperty"),1);out->size=1;out->bool_mask=1;return 1;}
    return 0;
}
void lifetime_call(void* object,void* fn,void* params) {
    if(object==&actor&&destroy_calls)++post_destroy_actor_events;
    check(object==&actor||(object==&foreign_owner&&fn==&get_level_fn)||(object==&statics&&fn==&census_fn),"retirement calls only qualified actor/owner or static world census");
    if(fn==&get_level_fn){++level_calls;auto result=object==&foreign_owner?&foreign_level:&level;std::memcpy(params,&result,sizeof(result));if(travel_on_get_level)current_world=&new_world;if(spline_garbage_after_call)actor.flags|=mirrored_garbage;}
    else if(fn==&destroy_fn){++destroy_calls;if(destroy_invalidates)actor.alive=false;if(destroy_garbage)actor.flags|=mirrored_garbage;if(travel_on_destroy)current_world=&new_world;}
    else if(fn==&tag_fn){static_cast<uint8_t*>(params)[8]=actor_persistent?1:0;}
    else if(fn==&owner_fn){std::memcpy(params,&retirement_owner,sizeof(retirement_owner));}
    else if(fn==&census_fn){
        const bool contains=actor.alive&&!(actor.flags&mirrored_garbage)&&!omit_world&&!(destroy_calls&&omit_after_destroy);
        Array a{};if(contains){a.data=std::malloc(sizeof(void*));check(a.data!=nullptr,"mock census allocation");a.count=1;a.capacity=1;auto pointer=&actor;std::memcpy(a.data,&pointer,sizeof(pointer));}
        std::memcpy(static_cast<uint8_t*>(params)+16,&a,sizeof(a));
        if(destroy_calls&&reuse_during_post_census)reuse_after_destroy=true;
        if(destroy_calls&&travel_during_post_census)current_world=&new_world;
        if(scope_reuse_during_census)actor.name=999;
        if(scope_travel_during_census)current_world=&new_world;
        if(scope_garbage_during_census)actor.flags|=mirrored_garbage;
        if(scope_disappear_during_census)actor.alive=false;
    }
    else throw std::runtime_error("unexpected lifetime function");
}
void* lifetime_world(const void* object) {return object==&gi?current_world:nullptr;}
void lifetime_reset(HsmpReflect& reflect) {
    for(auto c:{&meta,&world_class,&gi_class,&actor_class,&level_class,&function_class})c->cls=&meta;
    lifetime_objects={&meta,&world_class,&gi_class,&actor_class,&level_class,&function_class,&old_world,&new_world,&gi,&actor,&level,&get_level_fn,&destroy_fn,&driver_class,&tag_fn,&owner_fn,&replacement,&foreign_owner,&foreign_level,&statics_class,&statics,&census_fn};
    for(auto o:lifetime_objects){o->alive=true;o->flags=0;}
    old_world.property=&gi;new_world.property=&gi;level.property=&old_world;current_world=&old_world;
    foreign_level.property=&new_world;retirement_owner=nullptr;
    actor.cls=&actor_class;actor.name=113;
    travel_on_get_level=false;level_calls=0;destroy_calls=0;post_destroy_actor_touches=0;invalid_actor_resolves=0;
    actor_persistent=false;destroy_invalidates=true;reuse_after_destroy=false;travel_on_destroy=false;
    destroy_garbage=false;omit_world=false;omit_after_destroy=false;reuse_during_post_census=false;travel_during_post_census=false;post_destroy_actor_events=0;array_frees=0;
    actor_memory_unqualified=false;
    scope_reuse_during_census=false;scope_travel_during_census=false;scope_garbage_during_census=false;scope_disappear_during_census=false;
    identities.clear();names.clear();signatures.clear();lifetime_names.clear();mirrors.clear();
    retired_drivers.clear();
    active_guard=nullptr;active_world={};active_game_instance={};
    reflect.fname=lifetime_fname;reflect.find=lifetime_find;reflect.is_a=lifetime_is_a;reflect.class_of=lifetime_class;
    reflect.props=lifetime_props;reflect.obj_prop=lifetime_prop;reflect.call=lifetime_call;
    reflect.weak=lifetime_weak;reflect.resolve=lifetime_resolve;object_name=lifetime_name;object_world=lifetime_world;
    retirement_flags=lifetime_flags;retirement_free=lifetime_free;
    retirement_index=lifetime_index;retirement_object=lifetime_slot_object;retirement_serial=lifetime_slot_serial;
}
struct CreateMarker {std::string stage,name;uint32_t edge,marker,component,kind,function;uint64_t operation;};
std::vector<CreateMarker> create_markers;
void record_create(const char* stage,uint32_t edge,uint64_t operation,uint32_t marker,uint32_t component,
                   uint32_t kind,uint32_t function,const char* name){create_markers.push_back({stage,name,edge,marker,component,kind,function,operation});}
void create_trace_checks(HsmpReflect& reflect){
    create_markers.reserve(128);hsmp_presentation_set_create_log(nullptr);
    {CreateTrace disabled;disabled.part(1,0);disabled.emit("fixture",0);disabled.terminal(1);}
    check(create_markers.empty()&&!active_create_trace,"optional disabled create logger emits nothing and leaves no active scope");
    hsmp_presentation_set_create_log(record_create);create_markers.clear();const auto old_touches=touches;
    {CreateTrace trace;for(uint32_t i=1;i<=200;++i)trace.part(i,0);trace.terminal(1);}
    check(create_markers.size()==128&&create_markers[126].stage=="trace_limit"&&create_markers[127].stage=="create"&&create_markers[127].edge==1,
        "create trace explicitly exhausts before its reserved terminal and never exceeds128 markers");
    check(create_markers.back().component==200&&touches==old_touches,"suppressed transitions still retain terminal scalar context without engine metadata reads");
    create_markers.clear();
    {CreateTrace trace;for(uint32_t i=0;i<100;++i)if(trace.pe_enter(123,"GetLevel"))trace.emit("pe",1,123,"GetLevel");
        for(uint32_t i=1;i<=49;++i)trace.part(i,4);trace.terminal(1);}
    check(std::count_if(create_markers.begin(),create_markers.end(),[](const CreateMarker& m){return m.stage=="pe";})==64
        &&std::count_if(create_markers.begin(),create_markers.end(),[](const CreateMarker& m){return m.stage=="pe_limit";})==1
        &&std::count_if(create_markers.begin(),create_markers.end(),[](const CreateMarker& m){return m.stage=="component";})==49&&create_markers.size()<=128,
        "first32 engine call pairs are explicitly limited while all49 component transitions still fit the create budget");
    create_markers.clear();
    {CreateTrace trace;trace.part(7,9);char temporary[]="GetLevel";check(trace.pe_enter(123,temporary),"bounded PE entry starts an exact pair");
        trace.emit("pe",1,123,temporary);temporary[0]='X';trace.terminal(2);}
    check(create_markers.back().name=="GetLevel"&&create_markers.back().function==123&&create_markers.back().edge==2,
        "terminal owns a bounded function label rather than a destroyed Function or changed caller array");
    create_markers.clear();
    {CreateTrace outer;{CreateTrace inner;inner.terminal(1);}check(active_create_trace==&outer,"nested diagnostic scope restores original TLS context");outer.terminal(1);}
    lifetime_reset(reflect);create_markers.clear();int context=1;const HsmpViewGuard guard{&context,guard_check};
    {OperationScope scope(&guard,keep(&old_world));CreateTrace trace;trace.part(39,9);travel_on_get_level=true;
        rejects([&]{Function f(L"/Script/Engine.Actor:GetLevel");f.call(keep(&actor));},"post-PE world mutation still rejects under create diagnostics");trace.terminal(2);}
    check(create_markers.size()==5&&create_markers[2].stage=="pe"&&create_markers[2].edge==0&&create_markers[3].stage=="pe"&&create_markers[3].edge==1
        &&create_markers[3].name=="GetLevel"&&create_markers.back().edge==2,"PE exit is recorded immediately after dispatch before failing post-call world qualification");
    hsmp_presentation_set_create_log(nullptr);check(!active_create_trace,"finished create diagnostics do not leak into capture/apply frames");
}
void path_checks(HsmpReflect& reflect){
    lifetime_reset(reflect);lifetime_objects.push_back(&path_component);lifetime_objects.push_back(&path_package);
    path_component.name=126;path_component.alive=true;path_component.flags=0;path_component.cls=&actor_class;
    path_package.name=127;path_package.alive=true;path_package.flags=0;path_package.cls=&meta;
    path_component.outer=&actor;actor.outer=&level;level.outer=&old_world;old_world.outer=&path_package;path_package.outer=nullptr;
    source_outer=lifetime_outer;source_package_name=&path_package_name;path_package_name=700;path_mutate_name=path_mutate_class=path_mutate_package=false;
    const auto root=source_path_node(&path_component);std::array<HsmpNativePathNode,64> witness{};uint32_t count{};uint64_t package{};char reason[192]{};
    reflect.find=[](const uint16_t*)->void*{throw std::runtime_error("path witness must not search global objects");};
    reflect.call=[](void*,void*,void*){throw std::runtime_error("path witness must not dispatch ProcessEvent");};
    check(source_path_reader(&root,1,1,witness.data(),64,&count,&package,reason,sizeof(reason))==1&&count==5&&package==700,"complete original Outer chain captures through null without find or PE");
    auto verify=[&]{return source_path_reader(witness.data(),count,0,nullptr,0,nullptr,&package,reason,sizeof(reason));};
    check(verify()==1,"original hierarchy retains exact path witness");
    for(auto object:{&path_component,&actor,&level,&old_world,&path_package}){
        object->name^=1;check(verify()==-1,"every original Outer FName mutation refuses");object->name^=1;
        object->flags=mirrored_garbage;check(verify()==-1,"every original Outer native garbage state refuses");object->flags=0;
        object->alive=false;check(verify()==-1,"every original Outer weak disappearance refuses");object->alive=true;
    }
    actor_class.name^=1;check(verify()==-1,"original class FName discriminator mutation refuses");actor_class.name^=1;
    actor_class.flags=mirrored_garbage;check(verify()==-1,"original class native garbage refuses");actor_class.flags=0;
    path_component.cls=&level_class;check(verify()==-1,"original class address reuse refuses");path_component.cls=&actor_class;
    const auto saved_slot=lifetime_objects[root.weak-1];lifetime_objects[root.weak-1]=&replacement;check(verify()==-1,"original slot resolving to replacement address refuses");lifetime_objects[root.weak-1]=saved_slot;
    actor.outer=&path_package;check(verify()==-1,"intermediate reparent refuses");actor.outer=&level;
    path_package.outer=&meta;check(verify()==-1,"original null package boundary mutation refuses");path_package.outer=nullptr;
    path_package_name^=1;check(verify()==-1,"GPackageName formatter discriminator mutation refuses");path_package_name^=1;
    path_mutate_name=true;check(verify()==-1,"name change inside hard Outer getter refuses on post-read qualification");path_mutate_name=false;path_component.name=126;
    path_mutate_class=true;check(verify()==-1,"class name change inside hard Outer getter refuses on post-read qualification");path_mutate_class=false;actor_class.name^=1;
    path_mutate_package=true;check(verify()==-1,"formatter discriminator change during walk refuses");path_mutate_package=false;path_package_name=700;
    path_package.outer=&actor;check(source_path_reader(&root,1,1,witness.data(),64,&count,&package,reason,sizeof(reason))==-1&&count==0,"cycle refuses incomplete witness");path_package.outer=nullptr;
    std::array<LifetimeObject,65> bounded{};for(size_t i=0;i<bounded.size();++i){bounded[i].name=1000+i;bounded[i].cls=&meta;bounded[i].outer=i+1<bounded.size()?&bounded[i+1]:nullptr;lifetime_objects.push_back(&bounded[i]);}
    auto long_root=source_path_node(&bounded[0]);check(source_path_reader(&long_root,1,1,witness.data(),64,&count,&package,reason,sizeof(reason))==-1&&count==0,"65-node original hierarchy refuses instead of truncating");
    bounded[63].outer=nullptr;check(source_path_reader(&long_root,1,1,witness.data(),64,&count,&package,reason,sizeof(reason))==1&&count==64,"64-node original hierarchy captures complete within unchanged bound");
    source_outer=nullptr;check(source_path_reader(witness.data(),count,0,nullptr,0,nullptr,&package,reason,sizeof(reason))==-1,"missing original Outer export refuses");
    source_outer=lifetime_outer;source_package_name=nullptr;check(source_path_reader(witness.data(),count,0,nullptr,0,nullptr,&package,reason,sizeof(reason))==-1,"missing native package discriminator refuses");
    source_outer=nullptr;lifetime_reset(reflect);
}
// Original component memory and reflected array metadata exercise the actual
// bounded census. Non-null override values are deliberately invalid pointers:
// the provider must classify presence without following a color buffer.
struct VertexObject {LifetimeObject identity;std::array<uint8_t,0x600-sizeof(LifetimeObject)> storage{};};
static_assert(sizeof(VertexObject)==0x600);
VertexObject vertex_component_fixture{},vertex_second_fixture{};
LifetimeObject vertex_component_class{810,&meta},vertex_scene_class{811,&meta},vertex_actor_component_class{812,&meta};
LifetimeObject vertex_mesh_class{813,&meta},vertex_mesh{814,&vertex_mesh_class},vertex_mesh_other{815,&vertex_mesh_class};
LifetimeObject vertex_lod_struct{816,&meta},vertex_owner_function{817,&function_class};
LifetimeObject vertex_material_component_class{818,&meta};
LifetimeObject vertex_skeletal_probe_class{819,&meta};
struct VertexField {uint64_t key{},type{};int32_t bytes{},offset{};void* inner{},*structure{},*next{};};
VertexField vertex_array_field{},vertex_inner_field{};void* vertex_first_field{};
VertexField vertex_material_array_field{},vertex_material_inner_field{};void* vertex_material_first_field{};
std::array<uint8_t,16*0x90> vertex_lod_bytes{};
std::array<uint8_t,16*0x90> vertex_second_lods{};
int vertex_guard_calls{},vertex_mutation_at{},vertex_events{};
bool vertex_mutate_earlier_on_second{};
bool vertex_missing_asset_property{};
enum class VertexMutation {None,Override,Asset,Garbage,ClassName,Travel};
VertexMutation vertex_mutation{};
void vertex_write_header(Array header){std::memcpy(reinterpret_cast<uint8_t*>(&vertex_component_fixture)+0x588,&header,sizeof(header));}
void vertex_write_asset(LifetimeObject* value){std::memcpy(reinterpret_cast<uint8_t*>(&vertex_component_fixture)+0x560,&value,8);}
void vertex_write_override(size_t slot,uint64_t value){std::memcpy(vertex_lod_bytes.data()+slot*0x90+0x30,&value,8);}
int32_t vertex_guard_check(void*){
    ++vertex_guard_calls;
    if(vertex_mutation_at&&vertex_guard_calls==vertex_mutation_at){
        switch(vertex_mutation){
        case VertexMutation::Override:vertex_write_override(1,1);break;
        case VertexMutation::Asset:vertex_write_asset(&vertex_mesh_other);break;
        case VertexMutation::Garbage:vertex_component_fixture.identity.flags|=mirrored_garbage;break;
        case VertexMutation::ClassName:vertex_component_class.name^=1;break;
        case VertexMutation::Travel:current_world=&new_world;break;
        default:break;
        }
    }
    return 1;
}
void** vertex_children(void* object){return object==&vertex_material_component_class?&vertex_material_first_field:&vertex_first_field;}
void* vertex_next(void* p){return static_cast<VertexField*>(p)->next;}
void** vertex_inner(void* p){return &static_cast<VertexField*>(p)->inner;}
void** vertex_struct(void* p){return &static_cast<VertexField*>(p)->structure;}
int32_t* vertex_size(void* p){return &static_cast<VertexField*>(p)->bytes;}
int32_t* vertex_offset(void* p){return &static_cast<VertexField*>(p)->offset;}
SplineName* vertex_field_name(const void* p,SplineName* out){const auto v=static_cast<const VertexField*>(p)->key;std::memcpy(out,&v,8);return out;}
SplineVariant* vertex_field_class(void* p,SplineVariant* out){out->pointer=p;return out;}
SplineName* vertex_variant_name(const SplineVariant* p,SplineName* out){const auto v=static_cast<const VertexField*>(p->pointer)->type;std::memcpy(out,&v,8);return out;}
void* vertex_find(const uint16_t* key){
    const std::wstring path(reinterpret_cast<const wchar_t*>(key));
    if(path==L"/Script/Engine.StaticMeshComponent")return &vertex_component_class;
    if(path==L"/Script/Engine.SceneComponent")return &vertex_scene_class;
    if(path==L"/Script/Engine.ActorComponent")return &vertex_actor_component_class;
    if(path==L"/Script/Engine.StaticMesh")return &vertex_mesh_class;
    if(path==L"/Script/Engine.StaticMeshComponentLODInfo")return &vertex_lod_struct;
    if(path==L"/Script/Engine.MeshComponent")return &vertex_material_component_class;
    if(path==L"/Script/Engine.SkeletalMeshComponent")return &vertex_skeletal_probe_class;
    if(path==L"/Script/Engine.ActorComponent:GetOwner")return &vertex_owner_function;
    return lifetime_find(key);
}
int32_t vertex_is_a(void* object,void* type){
    if(object==&vertex_component_fixture||object==&vertex_second_fixture)return type==&vertex_component_class||type==&vertex_scene_class||type==&vertex_actor_component_class;
    return lifetime_is_a(object,type);
}
int32_t vertex_props(void* object,HsmpProp* out,int32_t cap,int32_t* size){
    if(object==&vertex_lod_struct){*size=vertex_inner_field.bytes;return 0;}
    if(object==&vertex_owner_function&&cap>0){*size=8;out[0]=object_field(L"ReturnValue",0);return 1;}
    return lifetime_props(object,out,cap,size);
}
int32_t vertex_prop(void* object,const uint16_t* key,HsmpProp* out){
    const std::wstring field(reinterpret_cast<const wchar_t*>(key));
    if((object==&vertex_component_fixture||object==&vertex_second_fixture)&&field==L"StaticMesh"){
        if(vertex_missing_asset_property)return 0;*out=object_field(L"StaticMesh",0x560);return 1;}
    if((object==&vertex_component_fixture||object==&vertex_second_fixture)&&field==L"LODData"){*out=object_field(L"LODData",0x588);out->cls=name(L"ArrayProperty");out->size=16;return 1;}
    if((object==&vertex_component_fixture||object==&vertex_second_fixture)&&field==L"OverrideMaterials"){*out=object_field(L"OverrideMaterials",0x518);out->cls=name(L"ArrayProperty");out->size=16;return 1;}
    return lifetime_prop(object,key,out);
}
void vertex_call(void* object,void* fn,void* params){
    ++vertex_events;
    if((object==&vertex_component_fixture||object==&vertex_second_fixture)&&fn==&vertex_owner_function){
        if(object==&vertex_second_fixture&&vertex_mutate_earlier_on_second)vertex_write_override(0,1);
        auto owner=&actor;std::memcpy(params,&owner,8);return;
    }
    lifetime_call(object,fn,params);
}
void vertex_reset(HsmpReflect& reflect){
    lifetime_reset(reflect);vertex_component_fixture={};vertex_component_fixture.identity.name=809;vertex_component_fixture.identity.cls=&vertex_component_class;
    vertex_second_fixture={};vertex_second_fixture.identity.name=808;vertex_second_fixture.identity.cls=&vertex_component_class;
    vertex_component_class.name=810;
    for(auto o:{&vertex_component_fixture.identity,&vertex_second_fixture.identity,&vertex_component_class,&vertex_scene_class,&vertex_actor_component_class,&vertex_mesh_class,&vertex_mesh,&vertex_mesh_other,&vertex_lod_struct,&vertex_owner_function,&vertex_material_component_class,&vertex_skeletal_probe_class}){
        o->flags=0;o->alive=true;lifetime_objects.push_back(o);
    }
    vertex_lod_bytes.fill(0);vertex_write_asset(&vertex_mesh);vertex_write_header({vertex_lod_bytes.data(),2,2});
    vertex_second_lods.fill(0);auto mesh=&vertex_mesh;Array second_header{vertex_second_lods.data(),2,2};
    std::memcpy(reinterpret_cast<uint8_t*>(&vertex_second_fixture)+0x560,&mesh,8);
    std::memcpy(reinterpret_cast<uint8_t*>(&vertex_second_fixture)+0x588,&second_header,sizeof(second_header));
    vertex_array_field={name(L"LODData"),name(L"ArrayProperty"),16,0x588,&vertex_inner_field,nullptr,nullptr};
    vertex_inner_field={0,name(L"StructProperty"),0x90,0,nullptr,&vertex_lod_struct,nullptr};vertex_first_field=&vertex_array_field;
    vertex_material_array_field={name(L"OverrideMaterials"),name(L"ArrayProperty"),16,0x518,&vertex_material_inner_field,nullptr,nullptr};
    vertex_material_inner_field={0,name(L"ObjectProperty"),8,0,nullptr,nullptr,nullptr};vertex_material_first_field=&vertex_material_array_field;
    reflect.find=vertex_find;reflect.is_a=vertex_is_a;reflect.props=vertex_props;reflect.obj_prop=vertex_prop;reflect.call=vertex_call;
    spline_api.children=vertex_children;spline_api.next=vertex_next;spline_api.inner=vertex_inner;spline_api.structure=vertex_struct;
    spline_api.size=vertex_size;spline_api.offset=vertex_offset;spline_api.field_name=vertex_field_name;spline_api.field_class=vertex_field_class;spline_api.variant_name=vertex_variant_name;
    vertex_flags=lifetime_flags;vertex_build_admit=[](){return true;};vertex_owner={};vertex_component={};vertex_asset={};
    vertex_guard_calls=vertex_mutation_at=vertex_events=0;vertex_mutation=VertexMutation::None;
    vertex_mutate_earlier_on_second=false;
    vertex_missing_asset_property=false;
}
void vertex_checks(HsmpReflect& reflect){
    check(vertex_code_ranges(vertex_count_code,sizeof(vertex_count_code),vertex_colors_code,sizeof(vertex_colors_code)),"both exact matched native code ranges admit");
    auto code=std::array<uint8_t,sizeof(vertex_colors_code)>{};std::copy_n(vertex_colors_code,code.size(),code.data());code.back()^=1;
    check(!vertex_code_ranges(vertex_count_code,sizeof(vertex_count_code),code.data(),code.size()),"changed override slot instruction refuses the shipping layout");
    check(!vertex_code_ranges(vertex_count_code,sizeof(vertex_count_code)-1,vertex_colors_code,sizeof(vertex_colors_code))&&!vertex_code_match(nullptr,0),"truncated or unavailable matched image refuses");
    auto observe=[&](HsmpViewVertexState& proof,HsmpViewResult& result){int context{};const HsmpViewGuard guard{&context,vertex_guard_check};return describe_vertex_state(keep(&old_world),keep(&actor),keep(&vertex_component_fixture),&guard,&proof,&result);};
    HsmpViewVertexState proof{};HsmpViewResult result{};
    vertex_reset(reflect);const auto baseline=observe(proof,result);if(baseline!=1)throw std::runtime_error(result.reason);
    check(baseline==1&&result.complete==1&&proof.lod_info_count==2&&proof.no_override==1&&proof.asset_present==1,"complete actual two-LOD null census proves native asset colors and observed presence");
    const int final_guard=vertex_guard_calls;check(vertex_events==4,"census retains both native owner and world qualification rounds");
    vertex_reset(reflect);vertex_write_override(1,1);check(observe(proof,result)==1&&proof.no_override==0&&proof.lod_info_count==2,"nonnull last original slot requires captured colors without buffer dereference");
    vertex_reset(reflect);vertex_write_header({nullptr,0,0});check(observe(proof,result)==1&&proof.lod_info_count==0&&proof.no_override==1,"complete empty native override array preserves actual asset color state");
    for(auto malformed:{Array{vertex_lod_bytes.data(),17,17},Array{vertex_lod_bytes.data(),-1,2},Array{vertex_lod_bytes.data(),2,1},Array{nullptr,2,2}}){
        vertex_reset(reflect);vertex_write_header(malformed);check(observe(proof,result)==-1&&result.complete==0,"malformed or overbound original array cannot prove vertex state");
    }
    vertex_reset(reflect);vertex_inner_field.bytes=0x80;check(observe(proof,result)==-1&&result.complete==0,"wrong reflected native inner stride refuses before slot interpretation");
    vertex_reset(reflect);vertex_array_field.offset=0x580;check(observe(proof,result)==-1,"wrong reflected component offset refuses");
    vertex_reset(reflect);vertex_build_admit=[](){return false;};check(observe(proof,result)==-1,"unsupported shipping code signature refuses");
    vertex_reset(reflect);vertex_mesh.flags=0x40;check(observe(proof,result)==-1&&vertex_events==0,"runtime transient asset cannot enter native asset proof");
    vertex_reset(reflect);vertex_component_fixture.identity.flags=mirrored_garbage;check(observe(proof,result)==-1&&vertex_events==0,"garbage original component receives no native qualifier dispatch");
    vertex_reset(reflect);vertex_component_class.flags=mirrored_garbage;check(observe(proof,result)==-1&&vertex_events==0,"garbage exact component class refuses before dispatch");
    for(auto mutation:{VertexMutation::Override,VertexMutation::Asset,VertexMutation::Garbage,VertexMutation::ClassName,VertexMutation::Travel}){
        vertex_reset(reflect);vertex_mutation=mutation;vertex_mutation_at=final_guard;
        check(observe(proof,result)==-1&&result.complete==0,"last callback mutation cannot pass a stale earlier slot census");
        check(vertex_guard_calls==final_guard,"final callback rejection performs no further guarded getter");
    }
    vertex_reset(reflect);auto world=keep(&old_world),owner=keep(&actor),component=keep(&vertex_component_fixture);HsmpViewComponent recipe_value{};recipe_value.kind=1;
    vertex_native_asset(world,owner,component,recipe_value,&result);check(true,"source and mirror native asset gate accepts complete null census");
    vertex_write_override(0,1);rejects([&]{vertex_native_asset(world,owner,component,recipe_value,&result);},"ongoing native asset frame gate rejects a newly introduced override");
    recipe_value.kind=0;rejects([&]{vertex_native_asset(world,owner,component,recipe_value,&result);},"unproven skeletal native asset slot layout is not inferred from static proof");
    vertex_reset(reflect);world=keep(&old_world);owner=keep(&actor);component=keep(&vertex_component_fixture);
    {VertexOperation operation(owner,component);Function call(L"/Script/Engine.ActorComponent:GetOwner");vertex_owner_function.flags=mirrored_garbage;
        rejects([&]{call.call(component);},"original native garbage function cannot dispatch through vertex operation");check(vertex_events==0,"garbage function produces zero ProcessEvent calls");}
    check(!vertex_owner.weak&&!vertex_component.weak&&!vertex_asset.weak,"borrowed vertex operation restores only scalar identities");
    auto finish_sources=[&](){int context{};const HsmpViewGuard guard{&context,vertex_guard_check};
        const auto owner=keep(&actor),mesh=keep(&vertex_mesh);
        const std::array<HsmpViewFinishTarget,2> targets{{{owner,keep(&vertex_component_fixture),mesh},{owner,keep(&vertex_second_fixture),mesh}}};
        return finish_scene_sets(keep(&old_world),targets.data(),2,nullptr,0,&guard,&result);};
    vertex_reset(reflect);check(finish_sources()==1&&result.complete==1,"complete source set accepts both original null component arrays");
    vertex_reset(reflect);vertex_mutate_earlier_on_second=true;
    check(finish_sources()==-1&&result.complete==0,"later source component callback cannot add an earlier override before whole-frame publication");
    auto finish_mirrors=[&](){int context{};const HsmpViewGuard guard{&context,vertex_guard_check};
        const auto world=keep(&old_world),owner=keep(&actor),mesh=keep(&vertex_mesh);
        Part first{};first.render=keep(&vertex_component_fixture);first.native_asset=mesh;
        Part second{};second.render=keep(&vertex_second_fixture);second.native_asset=mesh;
        mirrors.emplace(71,Mirror{world,owner,{first}});mirrors.emplace(72,Mirror{world,owner,{second}});
        const std::array<uint64_t,2> handles{71,72};return finish_scene_sets(world,nullptr,0,handles.data(),2,&guard,&result);};
    vertex_reset(reflect);check(finish_mirrors()==1&&result.complete==1,"all current mirrors pass whole-scene original native asset proof");
    vertex_reset(reflect);vertex_mutate_earlier_on_second=true;
    check(finish_mirrors()==-1&&result.complete==0,"later mirror callback cannot mutate an earlier native asset before scene readiness");
    vertex_flags=nullptr;vertex_build_admit=vertex_shipping_build;spline_api={};lifetime_reset(reflect);
}
// Synthetic original-class memory exercises the production scene helper. These
// copied layouts do not claim game rendering, camera ownership or live parity.
struct SceneObject {LifetimeObject identity;std::array<uint8_t,0xa00-sizeof(LifetimeObject)> storage{};};
SceneObject arm_first{},arm_second{},camera_fixture{};
struct SkeletalObject {LifetimeObject identity;std::array<uint8_t,0x1100-sizeof(LifetimeObject)> storage{};};
SkeletalObject skeletal_first{},skeletal_second{};
LifetimeObject skeletal_class{920,&meta},skinned_class{921,&meta},material_class{922,&meta};
LifetimeObject observed_material{923,&material_class},other_material{924,&material_class};
LifetimeObject material_get_fn{925,&function_class},material_set_fn{926,&function_class},component_world_fn{927,&function_class};
LifetimeObject skeletal_lod_struct{928,&meta};
LifetimeObject skeletal_deformer_struct{934,&meta};
VertexField skeletal_array_field{},skeletal_inner_field{};void* skeletal_first_field{};
VertexField skeletal_deformer_array{},skeletal_deformer_inner{};void* skeletal_deformer_first{};
std::array<uint8_t,16*0x28> skeletal_lods{},skeletal_second_lods{};
std::array<LifetimeObject*,32> skeletal_materials{},skeletal_second_materials{};
int skeletal_size{};
bool skeletal_asset_missing{},skeletal_wrong_owner{};
int skeletal_material_gets{},skeletal_material_sets{};
enum class SkeletalFnId {Add,Finish,BeginSpawn,FinishSpawn,ActorCollision,ActorTick,PostProcess,Anim,Cloth,Suspend,Suspended,Mesh,Leader,Deformer,Visibility,Mid,NumMaterials,Vector,Quat,Transform,Linear,Color,ParameterInfo,Count};
struct SkeletalFn {LifetimeObject object{};int32_t bytes{};std::vector<HsmpProp> fields;};
std::array<SkeletalFn,static_cast<size_t>(SkeletalFnId::Count)> skeletal_functions{};
std::map<std::wstring,LifetimeObject*> skeletal_function_paths;
LifetimeObject dynamic_material_class{929,&meta},dynamic_material{932,&dynamic_material_class},instance_material_class{933,&meta};
int skeletal_adds{},skeletal_finishes{},skeletal_mids{},skeletal_num_material_calls{},skeletal_leaders{};
bool skeletal_bad_null_set{},skeletal_make_active_after_material{};
enum class SkeletalMutation {None,Asset,Deprecated,MeshObject,Proxy,ProxyCopy,Override,Material,MaterialHeader,MaterialName,Name,ClassName,Garbage,Travel,Tick,Active,Leader,Animation,ClothAllow,ClothResume,PostProcess,ClothingInteractor};
SkeletalMutation skeletal_later_mutation{},skeletal_final_mutation{};int skeletal_mutation_at{};bool skeletal_mutation_applied{};
bool skeletal_object(const void* p){return p==&skeletal_first||p==&skeletal_second;}
void skeletal_pointer(SkeletalObject& object,size_t offset,uint64_t value){std::memcpy(reinterpret_cast<uint8_t*>(&object)+offset,&value,8);}
void skeletal_array(SkeletalObject& object,size_t offset,Array value){std::memcpy(reinterpret_cast<uint8_t*>(&object)+offset,&value,sizeof(value));}
void skeletal_mutate(SkeletalMutation change){
    switch(change){
    case SkeletalMutation::Asset:skeletal_pointer(skeletal_first,0x560,reinterpret_cast<uint64_t>(&vertex_mesh));break;
    case SkeletalMutation::Deprecated:skeletal_pointer(skeletal_first,0x558,reinterpret_cast<uint64_t>(&vertex_mesh));break;
    case SkeletalMutation::MeshObject:skeletal_pointer(skeletal_first,0x7a0,1);break;
    case SkeletalMutation::Proxy:skeletal_pointer(skeletal_first,0x2f0,1);break;
    case SkeletalMutation::ProxyCopy:skeletal_pointer(skeletal_first,0x4f8,1);break;
    case SkeletalMutation::Override:{uint64_t value=1;std::memcpy(skeletal_lods.data()+0x28+0x10,&value,8);break;}
    case SkeletalMutation::Material:skeletal_materials[1]=&other_material;break;
    case SkeletalMutation::MaterialHeader:skeletal_array(skeletal_first,0x518,{skeletal_materials.data(),1,2});break;
    case SkeletalMutation::MaterialName:observed_material.name^=1;break;
    case SkeletalMutation::Name:skeletal_first.identity.name^=1;break;
    case SkeletalMutation::ClassName:skeletal_class.name^=1;break;
    case SkeletalMutation::Garbage:skeletal_first.identity.flags|=mirrored_garbage;break;
    case SkeletalMutation::Travel:current_world=&new_world;break;
    case SkeletalMutation::Tick:reinterpret_cast<uint8_t*>(&skeletal_first)[0x3b]=1;break;
    case SkeletalMutation::Active:reinterpret_cast<uint8_t*>(&skeletal_first)[0x8a]|=8;break;
    case SkeletalMutation::Leader:skeletal_pointer(skeletal_first,0x568,lifetime_weak(&actor));break;
    case SkeletalMutation::Animation:skeletal_pointer(skeletal_first,0x8d0,reinterpret_cast<uint64_t>(&actor));break;
    case SkeletalMutation::ClothAllow:reinterpret_cast<uint8_t*>(&skeletal_first)[0xa42]|=2;break;
    case SkeletalMutation::ClothResume:reinterpret_cast<uint8_t*>(&skeletal_first)[0xa52]&=static_cast<uint8_t>(~8u);break;
    case SkeletalMutation::PostProcess:reinterpret_cast<uint8_t*>(&skeletal_first)[0xa41]&=static_cast<uint8_t>(~1u);break;
    case SkeletalMutation::ClothingInteractor:skeletal_pointer(skeletal_first,0xc40,reinterpret_cast<uint64_t>(&actor));break;
    default:break;
    }
}
void** skeletal_children(void* object){return object==&skinned_class?&skeletal_first_field:object==&skeletal_deformer_struct?&skeletal_deformer_first:vertex_children(object);}
LifetimeObject arm_class{900,&meta},camera_class{901,&meta};
LifetimeObject socket_fn{902,&function_class},deactivate_fn{903,&function_class};
LifetimeObject tick_fn{904,&function_class},active_fn{905,&function_class},tick_enabled_fn{906,&function_class};
LifetimeObject primitive_class{907,&meta},physics_fn{908,&function_class},collision_fn{909,&function_class};
LifetimeObject collision_read_fn{913,&function_class},simulating_fn{914,&function_class};
uint64_t scene_socket_value{};
bool scene_profile_supported{true},scene_wrong_vtable{},scene_wrong_size{},scene_wrong_debug_layout{},scene_wrong_tick_layout{},scene_wrong_getter{};
int scene_guards{},scene_events{},scene_mutation_at{};
int scene_primitive_events{};
enum class SceneMutation {None,Endpoint,Socket,Debug,Tick,Active,Garbage,ClassName,Travel};
SceneMutation scene_mutation{},scene_later_target_mutation{},scene_metadata_mutation{};
bool scene_mutation_applied{};
enum class EmptyMutation {None,Asset,Override,Name,Tick,Active};
EmptyMutation empty_later_mutation{},empty_final_mutation{};int empty_mutation_at{};bool empty_mutation_applied{};
void empty_mutate(EmptyMutation change){
    switch(change){
    case EmptyMutation::Asset:vertex_write_asset(&vertex_mesh);break;
    case EmptyMutation::Override:vertex_write_override(1,1);break;
    case EmptyMutation::Name:vertex_component_fixture.identity.name^=1;break;
    case EmptyMutation::Tick:reinterpret_cast<uint8_t*>(&vertex_component_fixture)[0x3b]=1;break;
    case EmptyMutation::Active:reinterpret_cast<uint8_t*>(&vertex_component_fixture)[0x8a]|=8;break;
    default:break;
    }
}
bool scene_object(const void* p){return p==&arm_first||p==&arm_second||p==&camera_fixture||p==&vertex_component_fixture||p==&vertex_second_fixture||skeletal_object(p);}
uint8_t* scene_bytes(SceneObject& object){return reinterpret_cast<uint8_t*>(&object);}
void scene_write_arm(SceneObject& object,const HsmpViewSpringArmFrame& value){
    std::memcpy(scene_bytes(object)+0x2f0,value.translation,24);std::memcpy(scene_bytes(object)+0x310,value.rotation,32);
}
void scene_mutate(SceneMutation change){
    switch(change){
    case SceneMutation::Endpoint:{auto value=arm_copy(&arm_first);value.translation[0]+=1;scene_write_arm(arm_first,value);break;}
    case SceneMutation::Socket:scene_socket_value^=1;break;
    case SceneMutation::Debug:scene_bytes(arm_first)[0x271]|=1;break;
    case SceneMutation::Tick:scene_bytes(arm_first)[0x3b]=1;break;
    case SceneMutation::Active:scene_bytes(arm_first)[0x8a]|=8;break;
    case SceneMutation::Garbage:arm_first.identity.flags|=mirrored_garbage;break;
    case SceneMutation::ClassName:arm_class.name^=1;break;
    case SceneMutation::Travel:current_world=&new_world;break;
    default:break;
    }
}
int32_t scene_guard_check(void*){
    ++scene_guards;if(scene_mutation_at&&scene_guards==scene_mutation_at)scene_mutate(scene_mutation);
    if(empty_mutation_at&&scene_guards==empty_mutation_at)empty_mutate(empty_final_mutation);
    if(skeletal_mutation_at&&scene_guards==skeletal_mutation_at)skeletal_mutate(skeletal_final_mutation);return 1;
}
uint64_t scene_fixture_vtable(const void* p){
    const auto* object=static_cast<const LifetimeObject*>(p);
    if(object->cls==&skeletal_class)return vertex_skeletal_image+0x7659cb0+(scene_wrong_vtable?8:0);
    return scene_image+(object->cls==&vertex_component_class?0x766fc60:object->cls==&camera_class?0x76085c0:0x76952b8)+(scene_wrong_vtable?8:0);
}
void* scene_find(const uint16_t* key){
    const std::wstring path(reinterpret_cast<const wchar_t*>(key));
    if(path==L"/Script/CoreUObject.Class")return &meta;
    if(path==L"/Script/Engine.CameraComponent")return &camera_class;
    if(path==L"/Script/Engine.SpringArmComponent")return &arm_class;
    if(path==L"/Script/Engine.SceneComponent:GetSocketTransform")return &socket_fn;
    if(path==L"/Script/Engine.ActorComponent:Deactivate")return &deactivate_fn;
    if(path==L"/Script/Engine.ActorComponent:SetComponentTickEnabled")return &tick_fn;
    if(path==L"/Script/Engine.ActorComponent:IsActive")return &active_fn;
    if(path==L"/Script/Engine.ActorComponent:IsComponentTickEnabled")return &tick_enabled_fn;
    if(path==L"/Script/Engine.PrimitiveComponent")return &primitive_class;
    if(path==L"/Script/Engine.PrimitiveComponent:SetSimulatePhysics")return &physics_fn;
    if(path==L"/Script/Engine.PrimitiveComponent:SetCollisionEnabled")return &collision_fn;
    if(path==L"/Script/Engine.PrimitiveComponent:GetCollisionEnabled")return &collision_read_fn;
    if(path==L"/Script/Engine.SceneComponent:IsSimulatingPhysics")return &simulating_fn;
    if(path==L"/Script/Engine.SkeletalMeshComponent")return &skeletal_class;
    if(path==L"/Script/Engine.SkinnedMeshComponent")return &skinned_class;
    if(path==L"/Script/Engine.SkelMeshComponentLODInfo")return &skeletal_lod_struct;
    if(path==L"/Script/Engine.MeshDeformerInstanceSet")return &skeletal_deformer_struct;
    if(path==L"/Script/Engine.MaterialInterface")return &material_class;
    if(path==L"/Script/Engine.MaterialInstanceDynamic")return &dynamic_material_class;
    if(path==L"/Script/Engine.MaterialInstance")return &instance_material_class;
    if(path==L"/Game/Test/ObservedMaterial.ObservedMaterial")return &observed_material;
    if(path==L"/Script/Engine.PrimitiveComponent:GetMaterial")return &material_get_fn;
    if(path==L"/Script/Engine.PrimitiveComponent:SetMaterial")return &material_set_fn;
    if(path==L"/Script/Engine.SceneComponent:K2_GetComponentToWorld")return &component_world_fn;
    const auto function=skeletal_function_paths.find(path);if(function!=skeletal_function_paths.end())return function->second;
    return vertex_find(key);
}
int32_t scene_is_a(void* object,void* type){
    if(object==&dynamic_material)return type==&dynamic_material_class||type==&instance_material_class||type==&material_class;
    if(scene_object(object)){auto original_class=static_cast<LifetimeObject*>(object)->cls;
        return type==original_class||type==&vertex_scene_class||type==&vertex_actor_component_class||
            (type==&primitive_class&&(original_class==&vertex_component_class||original_class==&skeletal_class))||
            (type==&skinned_class&&original_class==&skeletal_class);}
    return vertex_is_a(object,type);
}
HsmpProp scene_field(const wchar_t* key,const wchar_t* type,int32_t bytes,int32_t offset,uint8_t mask=0,const wchar_t* sub=nullptr){
    auto p=object_field(key,offset);p.cls=name(type);p.size=bytes;p.bool_mask=mask;if(sub)p.sub=name(sub);return p;
}
int32_t scene_props(void* object,HsmpProp* out,int32_t cap,int32_t* size){
    for(const auto& fn:skeletal_functions)if(object==&fn.object){*size=fn.bytes;if(cap<static_cast<int32_t>(fn.fields.size()))return -1;std::copy(fn.fields.begin(),fn.fields.end(),out);return static_cast<int32_t>(fn.fields.size());}
    if(object==&skeletal_class){*size=skeletal_size;return 0;}
    if(object==&skeletal_lod_struct){*size=skeletal_inner_field.bytes;if(cap>0){out[0]=object_field(L"OverrideVertexColors",0x10);return 1;}return 1;}
    if(object==&skeletal_deformer_struct){*size=0x20;if(cap>0){out[0]=scene_field(L"DeformerInstances",L"ArrayProperty",16,0);return 1;}return 1;}
    if((object==&material_get_fn||object==&material_set_fn)&&cap>=2){*size=16;out[0]=scene_field(L"ElementIndex",L"IntProperty",4,0);
        out[1]=scene_field(object==&material_get_fn?L"ReturnValue":L"Material",L"ObjectProperty",8,8);return 2;}
    if(object==&component_world_fn&&cap>0){*size=96;out[0]=scene_field(L"ReturnValue",L"StructProperty",96,0,0,L"Transform");return 1;}
    if(object==&vertex_component_class){*size=scene_wrong_size?0x5d0:0x5e0;return 0;}
    if(object==&camera_class||object==&arm_class){*size=scene_wrong_size?0x320:object==&camera_class?0x9e0:0x330;return 0;}
    if(object==&deactivate_fn){*size=0;return 0;}
    if(object==&tick_fn&&cap>0){*size=1;out[0]=scene_field(L"bEnabled",L"BoolProperty",1,0,1);return 1;}
    if(object==&physics_fn&&cap>0){*size=1;out[0]=scene_field(L"bSimulate",L"BoolProperty",1,0,1);return 1;}
    if(object==&collision_fn&&cap>0){*size=1;out[0]=scene_field(L"NewType",L"ByteProperty",1,0);return 1;}
    if(object==&collision_read_fn&&cap>0){*size=1;out[0]=scene_field(L"ReturnValue",L"ByteProperty",1,0);return 1;}
    if(object==&simulating_fn&&cap>=2){*size=16;out[0]=scene_field(L"BoneName",L"NameProperty",8,0);out[1]=scene_field(L"ReturnValue",L"BoolProperty",1,8,1);return 2;}
    if((object==&active_fn||object==&tick_enabled_fn)&&cap>0){*size=1;out[0]=scene_field(L"ReturnValue",L"BoolProperty",1,0,1);return 1;}
    if(object==&socket_fn&&cap>=3){*size=112;out[0]=scene_field(L"InSocketName",L"NameProperty",8,0);
        out[1]=scene_field(L"TransformSpace",L"ByteProperty",1,8);out[2]=scene_field(L"ReturnValue",L"StructProperty",96,16,0,L"Transform");return 3;}
    return vertex_props(object,out,cap,size);
}
int32_t scene_prop(void* object,const uint16_t* key,HsmpProp* out){
    const std::wstring field(reinterpret_cast<const wchar_t*>(key));
    if(object==&dynamic_material&&field==L"Parent"){*out=object_field(L"Parent",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    if(skeletal_object(object)){
        if(field==L"SkeletalMesh"||field==L"SkinnedAsset"){
            if(skeletal_asset_missing)return 0;*out=object_field(field.c_str(),field==L"SkeletalMesh"?0x558:0x560);return 1;}
        if(field==L"LODInfo"||field==L"OverrideMaterials"){
            *out=scene_field(field.c_str(),L"ArrayProperty",16,field==L"LODInfo"?0x760:0x518);return 1;}
        if(field==L"PhysicsAssetOverride"){*out=object_field(field.c_str(),0x738);return 1;}
        if(field==L"LeaderPoseComponent"){*out=scene_field(field.c_str(),L"WeakObjectProperty",8,0x568);return 1;}
        if(field==L"AnimBlueprintGeneratedClass"||field==L"AnimClass"){*out=scene_field(field.c_str(),L"ClassProperty",8,field==L"AnimClass"?0x8c8:0x8c0);return 1;}
        if(field==L"AnimScriptInstance"||field==L"PostProcessAnimInstance"||field==L"MeshDeformer"||field==L"ClothingInteractor"){
            *out=object_field(field.c_str(),field==L"AnimScriptInstance"?0x8d0:field==L"PostProcessAnimInstance"?0x8d8:field==L"MeshDeformer"?0x588:0xc40);return 1;}
        if(field==L"MeshDeformerInstances"){*out=scene_field(field.c_str(),L"StructProperty",0x20,0x598);return 1;}
        if(field==L"bSetMeshDeformer"){*out=scene_field(field.c_str(),L"BoolProperty",1,0x580,1);return 1;}
        if(field==L"bAllowClothActors"||field==L"bDisableClothSimulation"||field==L"bDisablePostProcessBlueprint"){
            *out=scene_field(field.c_str(),L"BoolProperty",1,field==L"bDisablePostProcessBlueprint"?0xa41:0xa42,field==L"bAllowClothActors"?2:field==L"bDisableClothSimulation"?4:1);return 1;}
    }
    if(scene_object(object)){
        if(field==L"bVisible"){*out=scene_field(field.c_str(),L"BoolProperty",1,0x1d0,1);return 1;}
        if(field==L"bHiddenInGame"){*out=scene_field(field.c_str(),L"BoolProperty",1,0x1d1,1);return 1;}
        if(field==L"bDrawDebugLagMarkers"){*out=scene_field(field.c_str(),L"BoolProperty",1,scene_wrong_debug_layout?0x270:0x271,1);return 1;}
        if(field==L"bIsActive"){*out=scene_field(field.c_str(),L"BoolProperty",1,0x8a,8);return 1;}
        if(field==L"PrimaryComponentTick"){
            *out=scene_field(field.c_str(),L"StructProperty",0x30,scene_wrong_tick_layout?0x38:0x30);
            if(object==&arm_first&&scene_metadata_mutation!=SceneMutation::None&&!scene_mutation_applied){scene_mutation_applied=true;scene_mutate(scene_metadata_mutation);}
            return 1;
        }
    }
    return vertex_prop(object,key,out);
}
void scene_call(void* object,void* fn,void* params){
    ++scene_events;
    for(size_t i=0;i<skeletal_functions.size();++i)if(fn==&skeletal_functions[i].object){
        const auto id=static_cast<SkeletalFnId>(i);auto* bytes=static_cast<uint8_t*>(params);
        if(id==SkeletalFnId::BeginSpawn){auto value=&actor;std::memcpy(bytes+128,&value,8);return;}
        if(id==SkeletalFnId::FinishSpawn){auto value=&actor;std::memcpy(bytes+112,&value,8);return;}
        if(id==SkeletalFnId::Add){++skeletal_adds;LifetimeObject* original_class{};std::memcpy(&original_class,bytes,8);check(original_class==&skeletal_class,"production create requests exact SkeletalMeshComponent without a pose leader");
            auto value=&skeletal_second;std::memcpy(bytes+112,&value,8);return;}
        if(id==SkeletalFnId::Finish){++skeletal_finishes;LifetimeObject* value{};std::memcpy(&value,bytes,8);check(value==&skeletal_second.identity,"production FinishAddComponent retains original created component");return;}
        if(id==SkeletalFnId::ActorCollision||id==SkeletalFnId::ActorTick){check(object==&actor&&!(bytes[0]&1),"owned mirror actor collision and tick are disabled");return;}
        if(id==SkeletalFnId::Deformer){LifetimeObject* value=nullptr;std::memcpy(bytes,&value,8);return;}
        if(id==SkeletalFnId::NumMaterials){++skeletal_num_material_calls;int32_t value=0;std::memcpy(bytes,&value,4);return;}
        if(id==SkeletalFnId::Visibility){static_cast<uint8_t*>(object)[0x1d0]=bytes[0]&1;return;}
        if(id==SkeletalFnId::Mid){++skeletal_mids;LifetimeObject* source{};int32_t slot{};std::memcpy(&slot,bytes,4);std::memcpy(&source,bytes+8,8);check(source==&observed_material&&slot==1,"nonnull material creation uses the observed source base and exact slot");
            dynamic_material.property=source;skeletal_second_materials[1]=&dynamic_material;skeletal_array(skeletal_second,0x518,{skeletal_second_materials.data(),2,32});auto value=&dynamic_material;std::memcpy(bytes+24,&value,8);return;}
        if(id==SkeletalFnId::Leader){++skeletal_leaders;LifetimeObject* value{};std::memcpy(&value,bytes,8);check(value==nullptr,"empty skeletal mirror receives no leader or replacement animation source");skeletal_pointer(skeletal_second,0x568,0);return;}
        if(id==SkeletalFnId::Mesh){LifetimeObject* value{};std::memcpy(&value,bytes,8);check(value==nullptr,"empty skeletal mirror keeps both asset aliases absent");skeletal_pointer(skeletal_second,0x558,0);skeletal_pointer(skeletal_second,0x560,0);return;}
        if(id==SkeletalFnId::Anim){LifetimeObject* value{};std::memcpy(&value,bytes,8);check(value==nullptr,"empty skeletal mirror sets no animation class");skeletal_pointer(skeletal_second,0x8c8,0);return;}
        if(id==SkeletalFnId::Cloth){check(!(bytes[0]&1),"empty skeletal mirror disables cloth actors");reinterpret_cast<uint8_t*>(&skeletal_second)[0xa42]&=static_cast<uint8_t>(~2u);return;}
        if(id==SkeletalFnId::Suspend){reinterpret_cast<uint8_t*>(&skeletal_second)[0xa52]|=8;return;}
        if(id==SkeletalFnId::Suspended){bytes[0]=(reinterpret_cast<uint8_t*>(object)[0xa52]&8)?1:0;return;}
        if(id==SkeletalFnId::PostProcess){check((bytes[0]&1)!=0,"empty skeletal mirror disables postprocess animation");reinterpret_cast<uint8_t*>(&skeletal_second)[0xa41]|=1;return;}
        throw std::runtime_error("unexpected skeletal creation function");
    }
    if(scene_object(object)){
        auto* p=static_cast<uint8_t*>(object);
        if(fn==&vertex_owner_function){
            if(object==&arm_second&&scene_later_target_mutation!=SceneMutation::None&&!scene_mutation_applied){scene_mutation_applied=true;scene_mutate(scene_later_target_mutation);}
            if(object==&vertex_second_fixture&&empty_later_mutation!=EmptyMutation::None&&!empty_mutation_applied){empty_mutation_applied=true;empty_mutate(empty_later_mutation);}
            if(object==&skeletal_second&&skeletal_later_mutation!=SkeletalMutation::None&&!skeletal_mutation_applied){skeletal_mutation_applied=true;skeletal_mutate(skeletal_later_mutation);}
            auto owner=skeletal_object(object)&&skeletal_wrong_owner?&foreign_owner:&actor;std::memcpy(params,&owner,8);return;
        }
        if(fn==&socket_fn){uint64_t socket{};std::memcpy(&socket,params,8);
            check(socket==scene_socket_value&&static_cast<uint8_t*>(params)[8]==2,"native socket read uses observed singleton and component space2");
            auto value=arm_copy(object);if(scene_wrong_getter)value.translation[0]+=1;
            const auto result=engine(arm_transform(value));std::memcpy(static_cast<uint8_t*>(params)+16,&result,sizeof(result));return;
        }
        if(fn==&deactivate_fn){p[0x8a]&=static_cast<uint8_t>(~8u);return;}
        if(fn==&tick_fn){p[0x3b]=static_cast<uint8_t*>(params)[0]&1;return;}
        if(fn==&active_fn){static_cast<uint8_t*>(params)[0]=(p[0x8a]&8)?1:0;return;}
        if(fn==&tick_enabled_fn){static_cast<uint8_t*>(params)[0]=p[0x3b]?1:0;return;}
        if(fn==&physics_fn){++scene_primitive_events;p[0x91]=static_cast<uint8_t*>(params)[0]&1;return;}
        if(fn==&collision_fn){++scene_primitive_events;p[0x90]=static_cast<uint8_t*>(params)[0];return;}
        if(fn==&collision_read_fn){++scene_primitive_events;static_cast<uint8_t*>(params)[0]=p[0x90];return;}
        if(fn==&simulating_fn){++scene_primitive_events;static_cast<uint8_t*>(params)[8]=p[0x91]?1:0;return;}
        if(skeletal_object(object)&&fn==&material_get_fn){int32_t index{};std::memcpy(&index,params,4);Array materials{};std::memcpy(&materials,p+0x518,sizeof(materials));
            ++skeletal_material_gets;
            check(index>=0&&index<32,"native material getter reads a bounded observed slot");auto value=index<materials.count?static_cast<LifetimeObject**>(materials.data)[index]:nullptr;std::memcpy(static_cast<uint8_t*>(params)+8,&value,8);return;}
        if(skeletal_object(object)&&fn==&material_set_fn){int32_t index{};LifetimeObject* value{};std::memcpy(&index,params,4);std::memcpy(&value,static_cast<uint8_t*>(params)+8,8);Array materials{};std::memcpy(&materials,p+0x518,sizeof(materials));
            ++skeletal_material_sets;
            check(index>=0&&index<32,"native material setter writes an observed slot");auto& slots=object==&skeletal_first?skeletal_materials:skeletal_second_materials;
            slots[static_cast<size_t>(index)]=skeletal_bad_null_set&&!value?&other_material:value;
            skeletal_array(*static_cast<SkeletalObject*>(object),0x518,{slots.data(),std::max(materials.count,index+1),32});
            if(skeletal_make_active_after_material)p[0x3b]=1;return;}
        if(fn==&component_world_fn){const auto value=engine(Transform{{1.25,-2.5,3},{0,0,0,1},{1,1,1}});std::memcpy(params,&value,sizeof(value));return;}
    }
    vertex_call(object,fn,params);
}
void scene_reset(HsmpReflect& reflect){
    vertex_reset(reflect);arm_first={};arm_second={};camera_fixture={};
    arm_first.identity.name=910;arm_first.identity.cls=&arm_class;arm_second.identity.name=911;arm_second.identity.cls=&arm_class;
    camera_fixture.identity.name=912;camera_fixture.identity.cls=&camera_class;arm_class.name=900;camera_class.name=901;
    for(auto object:{&arm_first.identity,&arm_second.identity,&camera_fixture.identity,&arm_class,&camera_class,&socket_fn,&deactivate_fn,&tick_fn,&active_fn,&tick_enabled_fn,&primitive_class,&physics_fn,&collision_fn,&collision_read_fn,&simulating_fn,&skeletal_class}){
        object->alive=true;object->flags=0;lifetime_objects.push_back(object);
    }
    const HsmpViewSpringArmFrame endpoint{{-0.0,3.125,-800.25},{0.0,-0.0,0.75,0.75}};
    scene_write_arm(arm_first,endpoint);scene_write_arm(arm_second,endpoint);
    scene_profile_supported=true;scene_wrong_vtable=scene_wrong_size=scene_wrong_debug_layout=scene_wrong_tick_layout=scene_wrong_getter=false;
    scene_guards=scene_events=scene_mutation_at=0;scene_mutation=scene_later_target_mutation=scene_metadata_mutation=SceneMutation::None;scene_mutation_applied=false;
    scene_primitive_events=0;
    empty_mutation_at=0;empty_later_mutation=empty_final_mutation=EmptyMutation::None;empty_mutation_applied=false;
    reflect.find=scene_find;reflect.is_a=scene_is_a;reflect.props=scene_props;reflect.obj_prop=scene_prop;reflect.call=scene_call;
    scene_image=0x10000000;scene_build_admit=[](){return scene_profile_supported;};scene_vtable_read=scene_fixture_vtable;
    vertex_empty_image=scene_image;vertex_empty_build_admit=[](){return scene_profile_supported;};vertex_empty_vtable_read=scene_fixture_vtable;vertex_empty=false;
    scene_socket_read=[](){return scene_socket_value;};scene_socket_value=name(L"OfflineExactSocket");scene_owner={};scene_component={};active_scene_kind=0;
}
void skeletal_reset(HsmpReflect& reflect){
    scene_reset(reflect);skeletal_first={};skeletal_second={};skeletal_size=0xf70;
    skeletal_first.identity={930,&skeletal_class};skeletal_second.identity={931,&skeletal_class};
    skeletal_class.name=920;skinned_class.name=921;material_class.name=922;observed_material.name=923;other_material.name=924;
    for(auto object:{&skeletal_first.identity,&skeletal_second.identity,&skinned_class,&material_class,&observed_material,&other_material,&material_get_fn,&material_set_fn,&component_world_fn,&skeletal_lod_struct,&skeletal_deformer_struct,&dynamic_material_class,&dynamic_material,&instance_material_class}){
        object->alive=true;object->flags=0;lifetime_objects.push_back(object);}
    skeletal_lods.fill(0);skeletal_second_lods.fill(0);skeletal_materials.fill(nullptr);skeletal_second_materials.fill(nullptr);
    skeletal_materials[1]=&observed_material;skeletal_second_materials[1]=&observed_material;
    skeletal_array(skeletal_first,0x760,{skeletal_lods.data(),2,2});skeletal_array(skeletal_second,0x760,{skeletal_second_lods.data(),2,2});
    skeletal_array(skeletal_first,0x518,{skeletal_materials.data(),3,32});skeletal_array(skeletal_second,0x518,{skeletal_second_materials.data(),3,32});
    skeletal_array_field={name(L"LODInfo"),name(L"ArrayProperty"),16,0x760,&skeletal_inner_field,nullptr,nullptr};
    skeletal_inner_field={0,name(L"StructProperty"),0x28,0,nullptr,&skeletal_lod_struct,nullptr};skeletal_first_field=&skeletal_array_field;
    skeletal_deformer_array={name(L"DeformerInstances"),name(L"ArrayProperty"),16,0,&skeletal_deformer_inner,nullptr,nullptr};
    skeletal_deformer_inner={0,name(L"ObjectProperty"),8,0,nullptr,nullptr,nullptr};skeletal_deformer_first=&skeletal_deformer_array;
    spline_api.children=skeletal_children;vertex_skeletal_image=scene_image;vertex_skeletal_build_admit=[](){return scene_profile_supported;};
    vertex_empty=false;vertex_kind=1;vertex_source_materials.clear();vertex_material_world={};
    skeletal_asset_missing=skeletal_wrong_owner=false;skeletal_material_gets=skeletal_material_sets=0;
    skeletal_later_mutation=skeletal_final_mutation=SkeletalMutation::None;skeletal_mutation_at=0;skeletal_mutation_applied=false;
    skeletal_adds=skeletal_finishes=skeletal_mids=skeletal_num_material_calls=skeletal_leaders=0;skeletal_bad_null_set=skeletal_make_active_after_material=false;
    skeletal_function_paths.clear();
    const auto function=[&](SkeletalFnId id,const wchar_t* path,int32_t bytes,std::initializer_list<HsmpProp> fields){
        const auto index=static_cast<size_t>(id);auto& fn=skeletal_functions[index];fn.object={10000+index,&function_class};fn.bytes=bytes;fn.fields=fields;
        lifetime_objects.push_back(&fn.object);skeletal_function_paths.emplace(path,&fn.object);};
    function(SkeletalFnId::Add,L"/Script/Engine.Actor:AddComponentByClass",120,{scene_field(L"Class",L"ClassProperty",8,0),scene_field(L"bManualAttachment",L"BoolProperty",1,8,1),scene_field(L"RelativeTransform",L"StructProperty",96,16,0,L"Transform"),scene_field(L"bDeferredFinish",L"BoolProperty",1,9,1),scene_field(L"ReturnValue",L"ObjectProperty",8,112)});
    function(SkeletalFnId::Finish,L"/Script/Engine.Actor:FinishAddComponent",112,{object_field(L"Component",0),scene_field(L"bManualAttachment",L"BoolProperty",1,8,1),scene_field(L"RelativeTransform",L"StructProperty",96,16,0,L"Transform")});
    function(SkeletalFnId::BeginSpawn,L"/Script/Engine.GameplayStatics:BeginDeferredActorSpawnFromClass",136,{object_field(L"WorldContextObject",0),scene_field(L"ActorClass",L"ClassProperty",8,8),scene_field(L"SpawnTransform",L"StructProperty",96,16,0,L"Transform"),scene_field(L"CollisionHandlingOverride",L"ByteProperty",1,112),object_field(L"Owner",120),scene_field(L"TransformScaleMethod",L"ByteProperty",1,113),object_field(L"ReturnValue",128)});
    function(SkeletalFnId::FinishSpawn,L"/Script/Engine.GameplayStatics:FinishSpawningActor",120,{object_field(L"Actor",0),scene_field(L"SpawnTransform",L"StructProperty",96,16,0,L"Transform"),scene_field(L"TransformScaleMethod",L"ByteProperty",1,8),object_field(L"ReturnValue",112)});
    function(SkeletalFnId::ActorCollision,L"/Script/Engine.Actor:SetActorEnableCollision",1,{scene_field(L"bNewActorEnableCollision",L"BoolProperty",1,0,1)});
    function(SkeletalFnId::ActorTick,L"/Script/Engine.Actor:SetActorTickEnabled",1,{scene_field(L"bEnabled",L"BoolProperty",1,0,1)});
    function(SkeletalFnId::PostProcess,L"/Script/Engine.SkeletalMeshComponent:SetDisablePostProcessBlueprint",1,{scene_field(L"bInDisablePostProcess",L"BoolProperty",1,0,1)});
    function(SkeletalFnId::Anim,L"/Script/Engine.SkeletalMeshComponent:SetAnimClass",8,{scene_field(L"NewClass",L"ClassProperty",8,0)});
    function(SkeletalFnId::Cloth,L"/Script/Engine.SkeletalMeshComponent:SetAllowClothActors",1,{scene_field(L"bInAllow",L"BoolProperty",1,0,1)});
    function(SkeletalFnId::Suspend,L"/Script/Engine.SkeletalMeshComponent:SuspendClothingSimulation",0,{});
    function(SkeletalFnId::Suspended,L"/Script/Engine.SkeletalMeshComponent:IsClothingSimulationSuspended",1,{scene_field(L"ReturnValue",L"BoolProperty",1,0,1)});
    function(SkeletalFnId::Mesh,L"/Script/Engine.SkeletalMeshComponent:SetSkeletalMeshAsset",8,{object_field(L"NewMesh",0)});
    function(SkeletalFnId::Leader,L"/Script/Engine.SkinnedMeshComponent:SetLeaderPoseComponent",16,{object_field(L"NewLeaderBoneComponent",0),scene_field(L"bForceUpdate",L"BoolProperty",1,8,1),scene_field(L"bInFollowerShouldTickPose",L"BoolProperty",1,9,1)});
    function(SkeletalFnId::Deformer,L"/Script/Engine.SkinnedMeshComponent:GetMeshDeformerInstance",8,{object_field(L"ReturnValue",0)});
    function(SkeletalFnId::Visibility,L"/Script/Engine.SceneComponent:SetVisibility",2,{scene_field(L"bNewVisibility",L"BoolProperty",1,0,1),scene_field(L"bPropagateToChildren",L"BoolProperty",1,1,1)});
    function(SkeletalFnId::Mid,L"/Script/Engine.PrimitiveComponent:CreateDynamicMaterialInstance",32,{scene_field(L"ElementIndex",L"IntProperty",4,0),object_field(L"SourceMaterial",8),scene_field(L"OptionalName",L"NameProperty",8,16),object_field(L"ReturnValue",24)});
    function(SkeletalFnId::NumMaterials,L"/Script/Engine.PrimitiveComponent:GetNumMaterials",4,{scene_field(L"ReturnValue",L"IntProperty",4,0)});
    const auto structure=[&](SkeletalFnId id,const wchar_t* path,int32_t bytes,std::initializer_list<HsmpProp> fields){function(id,path,bytes,fields);skeletal_functions[static_cast<size_t>(id)].object.cls=&meta;};
    structure(SkeletalFnId::Vector,L"/Script/CoreUObject.Vector",24,{scene_field(L"X",L"DoubleProperty",8,0),scene_field(L"Y",L"DoubleProperty",8,8),scene_field(L"Z",L"DoubleProperty",8,16)});
    structure(SkeletalFnId::Quat,L"/Script/CoreUObject.Quat",32,{scene_field(L"X",L"DoubleProperty",8,0),scene_field(L"Y",L"DoubleProperty",8,8),scene_field(L"Z",L"DoubleProperty",8,16),scene_field(L"W",L"DoubleProperty",8,24)});
    structure(SkeletalFnId::Transform,L"/Script/CoreUObject.Transform",96,{scene_field(L"Rotation",L"StructProperty",32,0,0,L"Quat"),scene_field(L"Translation",L"StructProperty",24,32,0,L"Vector"),scene_field(L"Scale3D",L"StructProperty",24,64,0,L"Vector")});
    structure(SkeletalFnId::Linear,L"/Script/CoreUObject.LinearColor",16,{scene_field(L"R",L"FloatProperty",4,0),scene_field(L"G",L"FloatProperty",4,4),scene_field(L"B",L"FloatProperty",4,8),scene_field(L"A",L"FloatProperty",4,12)});
    structure(SkeletalFnId::Color,L"/Script/CoreUObject.Color",4,{scene_field(L"B",L"ByteProperty",1,0),scene_field(L"G",L"ByteProperty",1,1),scene_field(L"R",L"ByteProperty",1,2),scene_field(L"A",L"ByteProperty",1,3)});
    structure(SkeletalFnId::ParameterInfo,L"/Script/Engine.MaterialParameterInfo",16,{scene_field(L"Name",L"NameProperty",8,0),scene_field(L"Association",L"ByteProperty",1,8),scene_field(L"Index",L"IntProperty",4,12)});
    layouts_verified=false;layout_objects.clear();
}
void skeletal_checks(HsmpReflect& reflect){
    HsmpViewResult result{};HsmpViewVertexState proof{};
    auto observe=[&]{int context{};const HsmpViewGuard guard{&context,scene_guard_check};
        return describe_vertex_state(keep(&old_world),keep(&actor),keep(&skeletal_first),&guard,&proof,&result);};
    skeletal_reset(reflect);
    check(observe()==1&&result.complete==1&&proof.component_kind==0&&proof.asset_present==0&&proof.no_override==1&&proof.lod_info_count==2&&proof.material_count==3&&proof.material_null_mask==5,
        "both hard-null native skeletal assets and complete mixed nullable material slots produce exact ABI11 proof");
    const int last_observe_guard=scene_guards;
    check(skeletal_material_gets==0,"raw complete material census does not depend on asset-derived GetNumMaterials or GetMaterial");
    skeletal_reset(reflect);skeletal_array(skeletal_first,0x518,{skeletal_materials.data(),32,32});
    check(observe()==1&&proof.material_count==32&&proof.material_null_mask==0xfffffffdu,"complete 32-slot material null mask retains high bit and actual nonnull slot");
    skeletal_reset(reflect);skeletal_array(skeletal_first,0x518,{nullptr,0,0});skeletal_array(skeletal_first,0x760,{nullptr,0,0});
    check(observe()==1&&proof.material_count==0&&proof.material_null_mask==0&&proof.lod_info_count==0,"observed empty native arrays remain empty rather than fabricated slots");
    skeletal_reset(reflect);skeletal_materials[1]=&actor;
    check(observe()==-1&&result.complete==0,"nonnull raw material slot requires an actual live MaterialInterface identity");
    skeletal_reset(reflect);observed_material.alive=false;
    check(observe()==-1&&result.complete==0,"unavailable original nonnull material cannot become a null slot");
    for(const auto offset:{size_t{0x558},size_t{0x560},size_t{0x7a0},size_t{0x2f0},size_t{0x4f8},size_t{0x738}}){skeletal_reset(reflect);skeletal_pointer(skeletal_first,offset,1);
        check(observe()==-1&&result.complete==0,"either asset alias, stale native render object/proxy or unsupported physics object refuses absence proof");}
    for(const auto header:{Array{skeletal_materials.data(),33,33},Array{skeletal_materials.data(),-1,32},Array{skeletal_materials.data(),3,2},Array{nullptr,3,3}}){skeletal_reset(reflect);skeletal_array(skeletal_first,0x518,header);
        check(observe()==-1&&result.complete==0,"malformed complete original material header cannot produce a null mask");}
    for(const auto header:{Array{skeletal_lods.data(),17,17},Array{skeletal_lods.data(),-1,2},Array{skeletal_lods.data(),2,1},Array{nullptr,2,2}}){skeletal_reset(reflect);skeletal_array(skeletal_first,0x760,header);
        check(observe()==-1&&result.complete==0,"skeletal absence never bypasses original LODInfo bounds");}
    skeletal_reset(reflect);uint64_t override_pointer=1;std::memcpy(skeletal_lods.data()+0x28+0x10,&override_pointer,8);
    check(observe()==1&&proof.component_kind==0&&proof.asset_present==0&&proof.no_override==0,"observed last skeletal LOD override is preserved without dereferencing an opaque buffer");
    HsmpViewComponent empty_gate{};empty_gate.kind=9;empty_gate.vertex_state=4;
    rejects([&]{vertex_native_asset(keep(&old_world),keep(&actor),keep(&skeletal_first),empty_gate,&result);},"empty skeletal geometry cannot admit a nonnull vertex override");
    for(int wrong=0;wrong<6;++wrong){skeletal_reset(reflect);
        if(wrong==0)skeletal_asset_missing=true;else if(wrong==1)skeletal_size=0xf60;else if(wrong==2)scene_wrong_vtable=true;
        else if(wrong==3)scene_profile_supported=false;else if(wrong==4)skeletal_inner_field.bytes=0x30;else skeletal_array_field.offset=0x768;
        check(observe()==-1&&result.complete==0,"missing asset schema, class/layout/vtable/code or inner metadata refuses explicit empty skeletal proof");}
    skeletal_reset(reflect);skeletal_first.identity.cls=&vertex_scene_class;
    check(observe()==-1&&result.complete==0,"plain Scene cannot substitute for native empty SkeletalMeshComponent");
    skeletal_reset(reflect);skeletal_wrong_owner=true;
    check(observe()==-1&&result.complete==0,"empty skeletal original owner/world must remain qualified");
    skeletal_reset(reflect);skeletal_deformer_inner.bytes=4;
    check(observe()==-1&&result.complete==0,"unproved inner deformer pointer stride cannot be treated as an empty instance set");
    skeletal_reset(reflect);skeletal_array(skeletal_first,0x598,{skeletal_materials.data(),1,1});
    check(observe()==-1&&result.complete==0,"nonempty native deformer instance set refuses empty skeletal rendering proof");
    skeletal_reset(reflect);reinterpret_cast<uint8_t*>(&skeletal_first)[0x580]=1;
    check(observe()==-1&&result.complete==0,"enabled reflected mesh-deformer bit refuses original absent surface proof");
    skeletal_reset(reflect);reinterpret_cast<uint8_t*>(&skeletal_first)[0x580]=0x80;
    check(observe()==1&&result.complete==1&&reinterpret_cast<uint8_t*>(&skeletal_first)[0x580]==0x80,"original unrelated boolean bits are preserved while the admitted deformer mask remains clear");
    for(auto mutation:{SkeletalMutation::Asset,SkeletalMutation::Deprecated,SkeletalMutation::MeshObject,SkeletalMutation::Proxy,SkeletalMutation::ProxyCopy,SkeletalMutation::Override,SkeletalMutation::Material,SkeletalMutation::MaterialHeader,SkeletalMutation::Name,SkeletalMutation::ClassName,SkeletalMutation::Garbage,SkeletalMutation::Travel}){
        skeletal_reset(reflect);skeletal_final_mutation=mutation;skeletal_mutation_at=last_observe_guard;
        check(observe()==-1&&result.complete==0,"final native callback cannot bless changed skeletal absence, material slots or original identity");
        check(scene_guards==last_observe_guard,"failed last skeletal proof performs no later callback");}
    const auto txt=[](const wchar_t* value)->HsmpViewText{return {u16(value),static_cast<uint32_t>(std::wcslen(value)),0};};
    const wchar_t component_path[]=L"/Script/Engine.SkeletalMeshComponent",material_path[]=L"/Game/Test/ObservedMaterial.ObservedMaterial";
    std::array<HsmpViewMaterial,3> materials{};for(uint32_t i=0;i<3;++i)materials[i].slot=i;materials[1].base=txt(material_path);
    HsmpViewComponent c{};c.id=1;c.kind=9;c.vertex_state=4;c.asset=txt(component_path);c.materials=materials.data();c.material_count=3;c.relative={{0,0,0},{0,0,0,1},{1,1,1}};
    recipe(c);check(true,"only exact empty skeletal recipe can explicitly retain mixed null and nonnull material slots");
    HsmpViewParameter invalid_null_parameter{};
    for(int wrong=0;wrong<4;++wrong){auto invalid=c;if(wrong==0)invalid.kind=8;else if(wrong==1)invalid.bone_count=1;
        else if(wrong==2)invalid.asset={};else{materials[0].scalars=&invalid_null_parameter;materials[0].scalar_count=1;}
        rejects([&]{recipe(invalid);},"empty skeletal recipe refuses wrong class, invented bones or parameters on explicit null material");materials[0].scalars=nullptr;materials[0].scalar_count=0;}
    skeletal_reset(reflect);Function num_materials(L"/Script/Engine.PrimitiveComponent:GetNumMaterials");num_materials.call(keep(&skeletal_first));
    check(num_materials.value<int32_t>(L"ReturnValue",L"IntProperty")==0&&observe()==1&&proof.material_count==3&&proof.material_null_mask==5,"asset-derived getter zero never erases raw observed nullable material slots");
    source_static(keep(&skeletal_first),c,&result);check(skeletal_material_gets==3,"source_static reads all original null and nonnull slots in order");
    HsmpViewFrame output{};output.world=c.relative;capture_values(keep(&skeletal_first),c,output,&result);
    check(skeletal_material_gets==6&&output.world.p[0]==1.25&&output.world.p[1]==-2.5&&output.bone_count==0,"actual value capture preserves native transform and empty geometry while rechecking every material slot");
    skeletal_materials[0]=&other_material;rejects([&]{source_static(keep(&skeletal_first),c,&result);},"explicit null source slot cannot hide a newly present material");
    skeletal_materials[0]=nullptr;skeletal_materials[1]=nullptr;rejects([&]{capture_values(keep(&skeletal_first),c,output,&result);},"missing observed nonnull source material refuses capture rather than becoming null");
    skeletal_reset(reflect);output={};output.world=c.relative;int context{};const HsmpViewGuard capture_guard{&context,scene_guard_check};
    check(capture(keep(&old_world),keep(&actor),keep(&skeletal_first),&c,&output,&capture_guard,&result)==1&&result.complete==1&&output.world.p[0]==1.25,"full production capture admits exact empty skeletal with complete mixed materials");
    skeletal_reset(reflect);auto* primitive=reinterpret_cast<uint8_t*>(&skeletal_first);primitive[0x90]=2;primitive[0x91]=1;collision_off(keep(&skeletal_first),&result);
    check(scene_primitive_events==4&&primitive[0x90]==0&&primitive[0x91]==0,"actual empty skeletal creation helper uses native Primitive collision and physics readback");
    const auto create_mirror=[&]{skeletal_second_materials.fill(nullptr);skeletal_array(skeletal_second,0x518,{nullptr,0,0});
        return create(keep(&old_world),&c,1,&capture_guard,&result);};
    skeletal_reset(reflect);const auto source_before=vertex_copy(&skeletal_first,0x760);const auto handle=create_mirror();
    check(handle!=0&&result.complete==1&&skeletal_adds==1&&skeletal_finishes==1&&skeletal_leaders==1,"production kind9 create builds one exact inert SkeletalMeshComponent without a pose leader");
    const auto created=vertex_copy(&skeletal_second,0x760);
    check(skeletal_material_sets==2&&skeletal_mids==1&&created.materials.count==3&&created.material_slots[0]==0&&created.material_slots[1]==reinterpret_cast<uint64_t>(&dynamic_material)&&created.material_slots[2]==0,"production null setters grow trailing raw slots and retain observed nonnull MID in order");
    check(vertex_equal(source_before,vertex_copy(&skeletal_first,0x760)),"owned kind9 creation never modifies the original authoritative skeletal holder");
    check(mirrors.at(handle).parts.size()==1&&!mirrors.at(handle).parts[0].leader.weak&&reinterpret_cast<uint8_t*>(&skeletal_second)[0x3b]==0&&(reinterpret_cast<uint8_t*>(&skeletal_second)[0x8a]&8)==0,"owned native empty mirror has no substitute render leader and remains inactive");
    const auto callback_count=scene_events;discard(handle);
    check(!vertex_source_materials.contains(lifetime_weak(&skeletal_second))&&scene_events==callback_count,"discard expires owned material original snapshots without invoking callbacks or touching the source");
    skeletal_reset(reflect);skeletal_bad_null_set=true;check(create_mirror()==0&&result.complete==0,"production create refuses a null setter whose actual GetMaterial result is nonnull");
    auto finish=[&](bool owned){int context{};const HsmpViewGuard guard{&context,scene_guard_check};const auto owner=keep(&actor),world=keep(&old_world);
        if(!owned){const std::array<HsmpViewFinishTarget,2> targets{{{owner,keep(&skeletal_first),{},9},{owner,keep(&skeletal_second),{},9}}};
            return finish_scene_sets(world,targets.data(),2,nullptr,0,&guard,&result);}
        Part first{};first.kind=9;first.render=keep(&skeletal_first);first.materials={{},keep(&observed_material),{}};
        Part second=first;second.render=keep(&skeletal_second);mirrors.emplace(84,Mirror{world,owner,{first,second}});const uint64_t mirror_handle=84;
        return finish_scene_sets(world,nullptr,0,&mirror_handle,1,&guard,&result);};
    auto bind=[&](bool owned=false){const std::vector<Obj> expected{{},keep(&observed_material),{}};
        if(owned)for(auto object:{&skeletal_first,&skeletal_second}){auto* raw=reinterpret_cast<uint8_t*>(object);raw[0xa52]=8;raw[0xa41]=1;}
        vertex_bind_source_materials(keep(&old_world),keep(&actor),keep(&skeletal_first),owned?1u:0u,owned?&expected:nullptr);
        vertex_bind_source_materials(keep(&old_world),keep(&actor),keep(&skeletal_second),owned?1u:0u,owned?&expected:nullptr);scene_guards=0;};
    skeletal_reset(reflect);auto* raw=reinterpret_cast<uint8_t*>(&skeletal_first);raw[0x3b]=1;raw[0x8a]=8;raw[0xa42]=2;skeletal_pointer(skeletal_first,0x8d0,reinterpret_cast<uint64_t>(&actor));bind();
    check(finish(false)==1&&result.complete==1&&raw[0x3b]==1&&raw[0x8a]==8&&raw[0xa42]==2&&vertex_copy(&skeletal_first,0x760).animation[2]==reinterpret_cast<uint64_t>(&actor),"source original empty skeletal holder retains native activation, animation and cloth flags while material identity is verified");
    skeletal_reset(reflect);bind(true);raw=reinterpret_cast<uint8_t*>(&skeletal_first);raw[0x3b]=1;raw[0x8a]=8;
    check(finish(true)==1&&result.complete==1&&raw[0x3b]==0&&(raw[0x8a]&8)==0,"owned exact empty skeletal mirror is inert and retains complete nullable material slots");const int last_finish_guard=scene_guards;
    for(auto mutation:{SkeletalMutation::Asset,SkeletalMutation::Deprecated,SkeletalMutation::MeshObject,SkeletalMutation::Proxy,SkeletalMutation::ProxyCopy,SkeletalMutation::Override,SkeletalMutation::Material,SkeletalMutation::MaterialHeader,SkeletalMutation::MaterialName,SkeletalMutation::Name,SkeletalMutation::ClassName,SkeletalMutation::Travel}){
        skeletal_reset(reflect);bind();skeletal_later_mutation=mutation;
        check(finish(false)==-1&&result.complete==0&&skeletal_mutation_applied,"later component callback cannot refresh an earlier original material or absence snapshot");}
    for(auto mutation:{SkeletalMutation::Asset,SkeletalMutation::Deprecated,SkeletalMutation::Override,SkeletalMutation::Material,SkeletalMutation::MaterialHeader,SkeletalMutation::MaterialName,SkeletalMutation::Name,SkeletalMutation::ClassName,SkeletalMutation::Travel,SkeletalMutation::Tick,SkeletalMutation::Active,SkeletalMutation::Leader,SkeletalMutation::Animation,SkeletalMutation::ClothAllow,SkeletalMutation::ClothResume,SkeletalMutation::PostProcess,SkeletalMutation::ClothingInteractor}){
        skeletal_reset(reflect);bind(true);skeletal_final_mutation=mutation;skeletal_mutation_at=last_finish_guard;
        check(finish(true)==-1&&result.complete==0,"final callback mutation of earlier owned skeletal material, asset or inert state refuses complete scene");
        check(scene_guards==last_finish_guard,"skeletal complete-set rejection has no subsequent callback");}
    vertex_skeletal_build_admit=vertex_skeletal_shipping_build;vertex_skeletal_image=0;vertex_empty=false;vertex_kind=1;
    vertex_source_materials.clear();vertex_material_world={};scene_build_admit=scene_shipping_profile;scene_vtable_read=scene_vtable;scene_socket_read=scene_socket_bits;
    vertex_empty_build_admit=vertex_empty_shipping_build;vertex_empty_vtable_read=vertex_empty_vtable;vertex_empty_image=0;scene_image=0;vertex_flags=nullptr;vertex_build_admit=vertex_shipping_build;spline_api={};lifetime_reset(reflect);
}
void scene_checks(HsmpReflect& reflect){
    const wchar_t socket_text[]=L"OfflineExactSocket";const HsmpViewText socket{u16(socket_text),static_cast<uint32_t>(std::wcslen(socket_text)),0};
    HsmpViewResult result{};auto observe=[&]{int context{};const HsmpViewGuard guard{&context,scene_guard_check};
        OperationScope operation(&guard,keep(&old_world));return arm_observe(keep(&old_world),keep(&actor),keep(&arm_first),socket,&result);};
    scene_reset(reflect);const auto raw=arm_copy(&arm_first);const auto captured=observe();
    check(arm_equal(raw,captured)&&std::signbit(captured.translation[0])&&std::signbit(captured.rotation[1]),"source arm cache preserves finite raw doubles and signed zero without normalization");
    check(scene_events>0&&!scene_owner.weak&&!scene_component.weak&&!active_scene_kind,"native getter is exercised and borrowed scene scope restores original state");
    scene_reset(reflect);rejects([&]{arm_socket({});},"missing native socket cannot become a guessed default");
    scene_socket_value^=1;rejects([&]{arm_socket(socket);},"different original singleton FName bits refuse");
    for(int invalid=0;invalid<5;++invalid){scene_reset(reflect);
        if(invalid==0)scene_profile_supported=false;else if(invalid==1)scene_wrong_vtable=true;else if(invalid==2)scene_wrong_size=true;
        else if(invalid==3)scene_wrong_debug_layout=true;else scene_bytes(arm_first)[0x271]=1;
        rejects([&]{observe();},"unverified image/vtable/layout/debug profile refuses native arm capture");}
    scene_reset(reflect);scene_wrong_getter=true;rejects([&]{observe();},"original cache and native socket output mismatch refuses");
    for(auto component:{&arm_first,&camera_fixture}){scene_reset(reflect);scene_bytes(*component)[0x3b]=1;scene_bytes(*component)[0x8a]=8;
        collision_off(keep(component),&result);
        check(scene_bytes(*component)[0x3b]==0&&scene_primitive_events==0,"create collision helper admits exact native Camera/Arm without primitive calls or Scene substitution");}
    scene_reset(reflect);arm_first.identity.flags=mirrored_garbage;rejects([&]{observe();},"original garbage arm refuses before getter");check(scene_events==0,"garbage original arm receives no native dispatch");
    scene_reset(reflect);arm_first.identity.cls=&camera_class;rejects([&]{observe();},"camera cannot impersonate exact native spring arm");
    for(int invalid=0;invalid<7;++invalid){auto bad=raw;if(invalid<3)bad.translation[invalid]=std::numeric_limits<double>::quiet_NaN();else bad.rotation[invalid-3]=std::numeric_limits<double>::infinity();
        rejects([&]{arm_valid(bad);},"every raw arm scalar rejects nonfinite values");}
    HsmpViewComponent recipe_value{};recipe_value.kind=7;recipe_value.vertex_state=4;recipe_value.visible=1;
    const wchar_t arm_path[]=L"/Script/Engine.SpringArmComponent",camera_path[]=L"/Script/Engine.CameraComponent";
    recipe_value.asset={u16(arm_path),static_cast<uint32_t>(std::wcslen(arm_path)),0};recipe_value.relative={{0,0,0},{0,0,0,1},{1,1,1}};recipe_value.spring_arm_socket=socket;
    HsmpViewFrame frame_value{};frame_value.world=recipe_value.relative;auto value=raw;frame_value.spring_arm=&value;frame(recipe_value,frame_value);check(true,"exact arm recipe requires raw endpoint frame");
    frame_value.spring_arm=nullptr;rejects([&]{frame(recipe_value,frame_value);},"arm frame presence cannot be omitted");frame_value.spring_arm=&value;
    recipe_value.kind=6;recipe_value.asset={u16(camera_path),static_cast<uint32_t>(std::wcslen(camera_path)),0};recipe_value.spring_arm_socket={};
    rejects([&]{frame(recipe_value,frame_value);},"camera cannot carry a spring arm payload");frame_value.spring_arm=nullptr;frame(recipe_value,frame_value);check(true,"actual camera has its own inert scene recipe");
    recipe_value.kind=4;rejects([&]{recipe(recipe_value);},"actual camera cannot be downgraded to plain Scene evidence");
    auto finish=[&](bool owned){int context{};const HsmpViewGuard guard{&context,scene_guard_check};const auto owner=keep(&actor),world=keep(&old_world);
        if(!owned){const std::array<HsmpViewFinishTarget,3> targets{{{owner,keep(&arm_first),{},7,0,socket,raw},{owner,keep(&arm_second),{},7,0,socket,raw},{owner,keep(&camera_fixture),{},6}}};
            return finish_scene_sets(world,targets.data(),3,nullptr,0,&guard,&result);}
        Part first{};first.kind=7;first.render=keep(&arm_first);first.arm=raw;first.arm_socket=socket_text;
        Part second=first;second.render=keep(&arm_second);Part camera{};camera.kind=6;camera.render=keep(&camera_fixture);
        mirrors.emplace(81,Mirror{world,owner,{first,camera}});mirrors.emplace(82,Mirror{world,owner,{second}});const std::array<uint64_t,2> handles{81,82};
        return finish_scene_sets(world,nullptr,0,handles.data(),2,&guard,&result);};
    scene_reset(reflect);scene_bytes(arm_first)[0x3b]=1;scene_bytes(arm_first)[0x8a]=8;
    check(finish(false)==1&&result.complete==1&&scene_bytes(arm_first)[0x3b]==1&&scene_bytes(arm_first)[0x8a]==8,"source set captures native outputs without deactivating authoritative arm");
    scene_reset(reflect);check(finish(true)==1&&result.complete==1,"complete owned mirror set verifies exact cameras and arm outputs");const int last_guard=scene_guards;
    for(auto mutation:{SceneMutation::Endpoint,SceneMutation::Socket,SceneMutation::Debug}){scene_reset(reflect);scene_later_target_mutation=mutation;
        check(finish(false)==-1&&result.complete==0&&scene_mutation_applied,"later native target callback invalidates earlier arm endpoint/socket/debug");}
    for(auto mutation:{SceneMutation::Endpoint,SceneMutation::Socket,SceneMutation::Debug,SceneMutation::Tick,SceneMutation::Active}){
        scene_reset(reflect);scene_mutation=mutation;scene_mutation_at=last_guard;
        check(finish(true)==-1&&result.complete==0,"final guard cannot alter earlier owned mirror output/activation/tick");
        check(scene_guards==last_guard,"no callback follows the failed final whole-scene raw census");}
    for(auto mutation:{SceneMutation::Tick,SceneMutation::Active}){scene_reset(reflect);scene_metadata_mutation=mutation;
        check(finish(true)==-1&&result.complete==0&&scene_mutation_applied,"metadata after native disabled getter cannot bless enabled mirror snapshot");}
    scene_reset(reflect);scene_wrong_tick_layout=true;check(finish(true)==-1&&result.complete==0,"unknown tick layout cannot prove owned mirror inertness");
    scene_build_admit=scene_shipping_profile;scene_vtable_read=scene_vtable;scene_socket_read=scene_socket_bits;scene_image=0;
    scene_owner={};scene_component={};active_scene_kind=0;vertex_flags=nullptr;vertex_build_admit=vertex_shipping_build;spline_api={};lifetime_reset(reflect);
}
void empty_checks(HsmpReflect& reflect){
    HsmpViewResult result{};HsmpViewVertexState proof{};
    auto observe=[&]{int context{};const HsmpViewGuard guard{&context,scene_guard_check};
        return describe_vertex_state(keep(&old_world),keep(&actor),keep(&vertex_component_fixture),&guard,&proof,&result);};
    scene_reset(reflect);vertex_write_asset(nullptr);
    check(observe()==1&&result.complete==1&&proof.asset_present==0&&proof.no_override==1&&proof.lod_info_count==2,
        "original hard-null static asset plus complete all-null override census is explicit empty proof");
    scene_reset(reflect);vertex_write_asset(nullptr);vertex_write_header({nullptr,0,0});
    check(observe()==1&&proof.asset_present==0&&proof.no_override==1&&proof.lod_info_count==0,"actual empty override array is complete without fabricated LOD data");
    scene_reset(reflect);vertex_write_asset(nullptr);vertex_missing_asset_property=true;
    check(observe()==-1&&result.complete==0,"unavailable asset property never becomes observed null presence");
    scene_reset(reflect);vertex_write_asset(reinterpret_cast<LifetimeObject*>(1));
    check(observe()==-1&&result.complete==0,"unqualified non-null asset cannot masquerade as native empty");
    scene_reset(reflect);vertex_write_asset(nullptr);vertex_write_override(1,1);
    check(observe()==1&&proof.asset_present==0&&proof.no_override==0,"complete provider preserves observed override presence even when original asset is null");
    HsmpViewComponent empty_gate{};empty_gate.kind=8;empty_gate.vertex_state=4;
    rejects([&]{vertex_native_asset(keep(&old_world),keep(&actor),keep(&vertex_component_fixture),empty_gate,&result);},"null asset with existing override cannot pass native empty admission");
    scene_reset(reflect);vertex_write_asset(nullptr);vertex_write_header({vertex_lod_bytes.data(),17,17});
    check(observe()==-1&&result.complete==0,"null asset cannot waive complete LODData bounds");
    scene_reset(reflect);vertex_write_asset(nullptr);vertex_component_fixture.identity.cls=&vertex_scene_class;
    check(observe()==-1&&result.complete==0,"plain Scene cannot impersonate exact native empty StaticMeshComponent");
    scene_reset(reflect);vertex_write_asset(nullptr);auto* primitive=reinterpret_cast<uint8_t*>(&vertex_component_fixture);primitive[0x90]=2;primitive[0x91]=1;
    collision_off(keep(&vertex_component_fixture),&result);
    check(scene_primitive_events==4&&primitive[0x90]==0&&primitive[0x91]==0,"native empty static keeps the actual Primitive collision/physics inert path");
    HsmpViewComponent c{};c.kind=8;c.vertex_state=4;c.visible=1;const wchar_t path[]=L"/Script/Engine.StaticMeshComponent";
    c.asset={u16(path),static_cast<uint32_t>(std::wcslen(path)),0};c.relative={{0,0,0},{0,0,0,1},{1,1,1}};
    HsmpViewMaterial material_value{};material_value.slot=2;const wchar_t material_path[]=L"/Game/Test/ObservedMaterial.ObservedMaterial";
    material_value.base={u16(material_path),static_cast<uint32_t>(std::wcslen(material_path)),0};c.materials=&material_value;c.material_count=1;recipe(c);
    check(true,"native empty primitive retains the actual material dictionary without a Scene substitution");
    HsmpViewFrame frame_value{};frame_value.world=c.relative;frame(c,frame_value);check(true,"native empty frame needs no guessed geometry or dynamic attachment payload");
    c.asset={};rejects([&]{recipe(c);},"logical empty asset cannot omit the private exact native class");
    c.asset={u16(path),static_cast<uint32_t>(std::wcslen(path)),0};c.kind=4;rejects([&]{recipe(c);},"empty StaticMeshComponent is not plain Scene evidence");c.kind=8;
    HsmpViewSpringArmFrame arm{};frame_value.spring_arm=&arm;rejects([&]{frame(c,frame_value);},"native empty component cannot carry a guessed arm endpoint");
    auto finish=[&](bool owned){int context{};const HsmpViewGuard guard{&context,scene_guard_check};const auto owner=keep(&actor),world=keep(&old_world);
        if(!owned){const std::array<HsmpViewFinishTarget,2> targets{{{owner,keep(&vertex_component_fixture),{},8},{owner,keep(&vertex_second_fixture),keep(&vertex_mesh)}}};
            return finish_scene_sets(world,targets.data(),2,nullptr,0,&guard,&result);}
        Part empty{};empty.kind=8;empty.render=keep(&vertex_component_fixture);Part populated{};populated.kind=1;populated.render=keep(&vertex_second_fixture);populated.native_asset=keep(&vertex_mesh);
        mirrors.emplace(83,Mirror{world,owner,{empty,populated}});const uint64_t handle=83;
        return finish_scene_sets(world,nullptr,0,&handle,1,&guard,&result);};
    scene_reset(reflect);vertex_write_asset(nullptr);auto* raw=reinterpret_cast<uint8_t*>(&vertex_component_fixture);raw[0x3b]=1;raw[0x8a]=8;
    check(finish(false)==1&&result.complete==1&&raw[0x3b]==1&&raw[0x8a]==8,"source empty primitive remains native active while its absence is proven");
    scene_reset(reflect);vertex_write_asset(nullptr);raw=reinterpret_cast<uint8_t*>(&vertex_component_fixture);raw[0x3b]=1;raw[0x8a]=8;
    check(finish(true)==1&&result.complete==1&&raw[0x3b]==0&&(raw[0x8a]&8)==0,"owned native empty mirror retains exact class with disabled activation and tick");const int final_guard=scene_guards;
    for(auto mutation:{EmptyMutation::Asset,EmptyMutation::Override,EmptyMutation::Name}){
        scene_reset(reflect);vertex_write_asset(nullptr);empty_later_mutation=mutation;
        check(finish(false)==-1&&result.complete==0&&empty_mutation_applied,"later target cannot replace an earlier empty source asset/override/original identity");}
    for(auto mutation:{EmptyMutation::Asset,EmptyMutation::Override,EmptyMutation::Name,EmptyMutation::Tick,EmptyMutation::Active}){
        scene_reset(reflect);vertex_write_asset(nullptr);empty_final_mutation=mutation;empty_mutation_at=final_guard;
        check(finish(true)==-1&&result.complete==0,"final callback cannot change native empty mirror binding/census/inert state");
        check(scene_guards==final_guard,"failed final empty census has no subsequent callback");}
    scene_build_admit=scene_shipping_profile;scene_vtable_read=scene_vtable;scene_socket_read=scene_socket_bits;scene_image=0;
    vertex_empty_build_admit=vertex_empty_shipping_build;vertex_empty_vtable_read=vertex_empty_vtable;vertex_empty_image=0;vertex_empty=false;
    scene_owner={};scene_component={};active_scene_kind=0;vertex_flags=nullptr;vertex_build_admit=vertex_shipping_build;spline_api={};lifetime_reset(reflect);
}
LifetimeObject pose_class{1200,&meta},pose_count_fn{1201,&function_class},pose_asset_fn{1203,&function_class};
SkeletalObject pose_level{};
alignas(16) std::array<EngineTransform,2> pose_calc_a{},pose_calc_b{},pose_render_a{},pose_render_b{},pose_input{};
std::vector<uint32_t> pose_stages;bool pose_callback_replace{},pose_callback_dirty{},pose_allocate_mesh_serial{};uint32_t pose_asset_reads{},pose_mesh_serial{};
uint32_t mesh_mutation{},mesh_path_finds{};bool mesh_mutation_done{},mesh_lookup_serial{};
uint64_t pose_weak(void* p){return lifetime_weak(p)|(p==&vertex_mesh?static_cast<uint64_t>(pose_mesh_serial)<<32:0);}
void* pose_resolve(uint64_t id){const auto index=static_cast<uint32_t>(id),serial=static_cast<uint32_t>(id>>32);auto* p=lifetime_resolve(index);
    return serial&&(p!=&vertex_mesh||serial!=pose_mesh_serial)?nullptr:p;}
void* pose_find(const uint16_t* key){const std::wstring path(reinterpret_cast<const wchar_t*>(key));
    if(path==L"/Script/Engine.PoseableMeshComponent")return &pose_class;
    if(path==L"/Script/Engine.SkeletalMesh")return &vertex_mesh_class;
    if(path==L"/Game/ObservedClothing.ObservedClothing"){++mesh_path_finds;if(mesh_lookup_serial&&mesh_path_finds==2)pose_mesh_serial=7;return &vertex_mesh;}
    if(path==L"/Script/Engine.SkinnedMeshComponent:GetSkinnedAsset")return &pose_asset_fn;
    if(path==L"/Script/Engine.SkinnedMeshComponent:GetNumBones")return &pose_count_fn;return scene_find(key);}
int32_t pose_props(void* object,HsmpProp* out,int32_t cap,int32_t* bytes){
    if(object==&pose_class){*bytes=0xa20;return 0;}
    if(object==&pose_count_fn){*bytes=4;if(cap){out[0]=scene_field(L"ReturnValue",L"IntProperty",4,0);return 1;}return -1;}
    if(object==&pose_asset_fn){*bytes=8;if(cap){out[0]=object_field(L"ReturnValue",0);return 1;}return -1;}
    return scene_props(object,out,cap,bytes);}
int32_t pose_prop(void* object,const uint16_t* key,HsmpProp* out){
    if(object==&pose_level&&std::wstring(reinterpret_cast<const wchar_t*>(key))==L"OwningWorld"){*out=object_field(L"OwningWorld",0xc0);return 1;}return scene_prop(object,key,out);}
uint64_t pose_vtable(const void* p){if(p==&skeletal_second)return pose_image+0x7646b38;return scene_fixture_vtable(p);}
int32_t pose_is_a(void* object,void* type){if(object==&skeletal_second&&type==&skinned_class)return 1;return scene_is_a(object,type);}
void pose_event(void* object,void* fn,void* params){
    if(fn==&get_level_fn){auto value=&pose_level;std::memcpy(params,&value,8);return;}
    if(fn==&pose_asset_fn){++pose_asset_reads;if(pose_allocate_mesh_serial)pose_mesh_serial=9;uint64_t value{};std::memcpy(&value,static_cast<uint8_t*>(object)+0x558,8);if(!value)std::memcpy(&value,static_cast<uint8_t*>(object)+0x560,8);std::memcpy(params,&value,8);
        if(mesh_mutation&&!mesh_mutation_done){mesh_mutation_done=true;
            if(mesh_mutation==1)vertex_mesh.name^=1;else if(mesh_mutation==2)vertex_mesh.cls=&material_class;
            else if(mesh_mutation==3)vertex_mesh.flags=mirrored_garbage;else if(mesh_mutation==4)vertex_mesh.outer=&actor;
            else if(mesh_mutation==5)skeletal_pointer(pose_level,0xc0,reinterpret_cast<uint64_t>(&new_world));
            else if(mesh_mutation==6){const auto index=static_cast<uint32_t>(lifetime_weak(&vertex_mesh));lifetime_objects[index-1]=&vertex_mesh_other;}
            else if(mesh_mutation==7)skeletal_pointer(*static_cast<SkeletalObject*>(object),0x560,reinterpret_cast<uint64_t>(&vertex_mesh_other));
            else if(mesh_mutation==8)vertex_mesh.flags=0x40;}
        return;}
    if(fn==&pose_count_fn){const int32_t value=2;std::memcpy(params,&value,4);return;}scene_call(object,fn,params);}
void pose_action(Obj component,uint32_t stage){
    pose_stages.push_back(stage);auto* object=reinterpret_cast<SkeletalObject*>(component.address);auto* raw=reinterpret_cast<uint8_t*>(object);
    if(stage==0){auto buffers=pose_buffers(object,2);std::memcpy(buffers.arrays[buffers.editable].data,pose_input.data(),sizeof(pose_input));
        const auto old=buffers.editable;std::memcpy(raw+0x610,&old,4);const int32_t next=1-old;std::memcpy(raw+0x60c,&next,4);raw[0x798]&=static_cast<uint8_t>(~0x40u);}
    if(stage==1){check((raw[0x798]&0x40)!=0,"production transfer marks native needs-flip before finalize");
        auto buffers=pose_buffers(object,2);std::memcpy(raw+0x610,&buffers.editable,4);std::memcpy(raw+0x60c,&buffers.read,4);raw[0x798]&=static_cast<uint8_t>(~0x40u);}
    if(stage==5&&pose_callback_replace)skeletal_pointer(skeletal_first,0x558,reinterpret_cast<uint64_t>(&vertex_mesh_other));
    if(stage==5&&pose_callback_dirty)raw[0x798]|=0x40;
}
void pose_reset(HsmpReflect& reflect){
    skeletal_reset(reflect);pose_class={1200,&meta};pose_count_fn={1201,&function_class};pose_asset_fn={1203,&function_class};pose_level={};pose_level.identity={1202,&level_class};
    lifetime_objects.push_back(&pose_class);lifetime_objects.push_back(&pose_count_fn);lifetime_objects.push_back(&pose_asset_fn);lifetime_objects.push_back(&pose_level.identity);actor.outer=&pose_level.identity;
    skeletal_pointer(pose_level,0xc0,reinterpret_cast<uint64_t>(&old_world));skeletal_second.identity.cls=&pose_class;
    reflect.find=pose_find;reflect.is_a=pose_is_a;reflect.props=pose_props;reflect.obj_prop=pose_prop;reflect.call=pose_event;reflect.weak=pose_weak;reflect.resolve=pose_resolve;source_outer=lifetime_outer;
    pose_image=scene_image;pose_build_admit=[](){return true;};scene_vtable_read=pose_vtable;pose_native_call=pose_action;
    pose_calc_a={};pose_calc_b={};pose_render_a={};pose_render_b={};pose_input={};pose_stages.clear();pose_callback_replace=pose_callback_dirty=pose_allocate_mesh_serial=mesh_lookup_serial=false;pose_asset_reads=pose_mesh_serial=mesh_mutation=mesh_path_finds=0;mesh_mutation_done=false;
    vertex_mesh.name=814;vertex_mesh.cls=&vertex_mesh_class;vertex_mesh.outer=&path_package;source_package_name=&path_package_name;path_package_name=700;
    path_package={127,&meta};lifetime_objects.push_back(&path_package);
    for(size_t i=0;i<2;++i){Transform value{{static_cast<double>(i)+1,2,3},{0,0,0,1},{1,1,1}};pose_input[i]=engine(value);}
    for(auto* object:{&skeletal_first,&skeletal_second}){skeletal_pointer(*object,0x558,reinterpret_cast<uint64_t>(&vertex_mesh));skeletal_pointer(*object,0x560,reinterpret_cast<uint64_t>(&vertex_mesh));
        skeletal_pointer(*object,0x90,reinterpret_cast<uint64_t>(&actor));auto* p=reinterpret_cast<uint8_t*>(object);p[0x798]=0x20;p[0x88]=3;p[0xa41]=1;p[0xa52]=8;
        const int32_t editable=0,read_index=1;std::memcpy(p+0x60c,&editable,4);std::memcpy(p+0x610,&read_index,4);}
    skeletal_array(skeletal_second,0x5c8,{pose_calc_a.data(),2,2});skeletal_array(skeletal_second,0x5d8,{pose_calc_b.data(),2,2});
    skeletal_array(skeletal_second,0x8b8,{pose_input.data(),2,2});
    skeletal_array(skeletal_first,0x5c8,{pose_render_a.data(),2,2});skeletal_array(skeletal_first,0x5d8,{pose_render_b.data(),2,2});
}
void pose_checks(HsmpReflect& reflect){
    HsmpViewResult result{};const auto binding=[&](){return pose_bind(keep(&old_world),keep(&actor),keep(&skeletal_second),keep(&skeletal_first),keep(&vertex_mesh),2,&result);};
    pose_reset(reflect);HsmpViewComponent readback_recipe{};readback_recipe.id=6;readback_recipe.kind=0;readback_recipe.parent=10;
    const wchar_t socket[]=L"ObservedSocket";readback_recipe.socket={u16(socket),static_cast<uint32_t>(std::wcslen(socket)),0};
    Transform expected_world{{1.25,-2.5,3},{0,0,0,1},{1,1,1}};world_readback(keep(&skeletal_first),readback_recipe,expected_world,"world_set",&result);
    check(result.operations==1,"matching world readback retains one original guarded native getter");expected_world.p[1]+=2;expected_world.q[0]=0.5;expected_world.scale[2]+=0.125;
    bool precise_world{};try{world_readback(keep(&skeletal_first),readback_recipe,expected_world,"complete",&result);}catch(const Error& e){const std::string reason=e.what();
        precise_world=reason.find("stage=complete id=6 kind=0 parent=10")!=std::string::npos&&reason.find("dp=2 dq=0.5 ds=0.125 qe=0.5 socket=ObservedSocket")!=std::string::npos&&reason.size()<sizeof(result.reason);}
    check(precise_world&&result.operations==2,"world refusal reports original component/parent/socket and raw double deltas without changing tolerance or adding getters");
    const wchar_t requested[]=L"/Game/ObservedClothing.ObservedClothing";const HsmpViewText requested_path{u16(requested),static_cast<uint32_t>(std::wcslen(requested)),0};
    pose_reset(reflect);mesh_assignment(keep(&skeletal_first),keep(&vertex_mesh),requested_path,false,"mesh_set",&result);
    mesh_assignment(keep(&skeletal_second),keep(&vertex_mesh),requested_path,true,"verified",&result);check(pose_asset_reads==2,"both exact assigned mesh identities still pass their native getters");
    for(bool calculator:{false,true})for(bool absent:{false,true}){pose_reset(reflect);auto& target=calculator?skeletal_second:skeletal_first;
        const auto actual=absent?0:reinterpret_cast<uint64_t>(&vertex_mesh_other);skeletal_pointer(target,0x558,actual);skeletal_pointer(target,0x560,actual);bool exact{};
        try{mesh_assignment(keep(&target),keep(&vertex_mesh),requested_path,calculator,"mesh_set",&result);}catch(const Error& e){const std::string reason=e.what();
            exact=reason.find(calculator?"mirror pose calculator mesh assignment failed":"mirror render mesh assignment failed")==0&&
                reason.find("stage=mesh_set")!=std::string::npos&&reason.find(absent?"actual=absent":"actual=different")!=std::string::npos&&reason.find("address_equal=0; index_equal=0; zero_to_nonzero=0")!=std::string::npos&&reason.find("asset=ObservedClothing")!=std::string::npos&&reason.size()<sizeof(result.reason);}
        check(exact,"mesh assignment refusal names the exact target and observed missing/different identity without native metadata queries");
        check(pose_asset_reads==1,"failed assignment performs one original native getter without checking the other target");}
    pose_reset(reflect);const auto original_mesh=keep(&vertex_mesh);pose_allocate_mesh_serial=true;bool serial_detail{};
    try{mesh_assignment(keep(&skeletal_first),original_mesh,requested_path,false,"mesh_set",&result);}catch(const Error& e){serial_detail=std::string(e.what()).find("address_equal=1; index_equal=1; zero_to_nonzero=1")!=std::string::npos;}
    check(serial_detail,"same original address/index with newly allocated native serial is distinguished without accepting the changed weak identity");
    check(pose_asset_reads==1,"serial mismatch diagnostic uses only the already-qualified getter result and original copied identity");
    pose_reset(reflect);auto admitted=mesh_binding(keep(&old_world),keep(&actor),requested_path,&result);const auto original_zero=admitted.original;
    pose_allocate_mesh_serial=true;const auto promoted=mesh_admit(admitted,keep(&skeletal_first),requested_path,false,"mesh_set",&result);
    check((promoted.weak>>32)==9&&admitted.pinned.weak==promoted.weak&&admitted.original.weak==original_zero.weak&&!same(promoted,original_zero),
        "qualified natural serial allocation pins positive identity while retaining original zero identity and strict global equality");
    auto positive_pose=binding();check(positive_pose.asset.weak==promoted.weak,"the actual positive asset pin propagates into later native pose binding");mesh_binding_final(admitted);
    pose_mesh_serial=10;rejects([&]{mesh_binding_final(admitted);},"retained positive asset serial can never be re-pinned after another serial change");
    pose_reset(reflect);mesh_lookup_serial=true;admitted=mesh_binding(keep(&old_world),keep(&actor),requested_path,&result);
    check((admitted.original.weak>>32)==0&&(admitted.pinned.weak>>32)==7,"binding pins the first positive serial observed by its bracketed exact lookup");
    pose_mesh_serial=9;rejects([&]{mesh_admit(admitted,keep(&skeletal_first),requested_path,false,"mesh_set",&result);},
        "a later positive serial cannot replace one already observed and pinned during original recipe binding");
    for(uint32_t mutation=1;mutation<=8;++mutation){pose_reset(reflect);admitted=mesh_binding(keep(&old_world),keep(&actor),requested_path,&result);
        pose_allocate_mesh_serial=true;mesh_mutation=mutation;rejects([&]{mesh_admit(admitted,keep(&skeletal_first),requested_path,false,"mesh_set",&result);},
            "original rename/class/garbage/path/world/slot reuse/alias/transient callback changes refuse before positive pin");
        check((admitted.pinned.weak>>32)==0,"failed callback closure never promotes the original asset ticket");}
    pose_reset(reflect);admitted=mesh_binding(keep(&old_world),keep(&actor),requested_path,&result);pose_allocate_mesh_serial=true;
    mesh_admit(admitted,keep(&skeletal_first),requested_path,false,"mesh_set",&result);Mirror witnessed{keep(&old_world),keep(&actor),{}};Part retained{};retained.mesh=admitted;witnessed.parts.push_back(retained);
    {MeshWatch watch({&witnessed});vertex_mesh.name^=1;rejects([&]{check_guard();},"later component callback rechecks the earlier retained mesh path before continuing");vertex_mesh.name^=1;}
    mirror_mutex.lock();mirrors.emplace(88,std::move(witnessed));mirror_mutex.unlock();vertex_mesh.outer=&actor;
    const uint64_t retained_handle=88;int context=1;const HsmpViewGuard retained_guard{&context,guard_check};
    check(finish_scene_sets(keep(&old_world),nullptr,0,&retained_handle,1,&retained_guard,&result)==-1,
        "all-owned complete-scene finish includes the original nonempty skeletal asset witness");
    pose_reset(reflect);auto b=binding();auto output=pose_transfer(b,&result);pose_final(b,output);
    check(pose_stages==std::vector<uint32_t>({0,1,2,3,4,5}),"native calculator refresh precedes complete transfer/finalize/children/bounds/dirty notifications");
    check(std::memcmp(output.values.data(),pose_input.data(),sizeof(pose_input))==0&&std::memcmp(pose_render_a.data(),pose_input.data(),sizeof(pose_input))==0,
        "production pose transfer preserves every complete component-space transform");
    check(pose_render_b[0].p[0]==0&&reinterpret_cast<uint8_t*>(&skeletal_first)[0x3b]==0,"previous engine read allocation and inert tick remain intact");
    for(int fault=0;fault<4;++fault){pose_reset(reflect);b=binding();if(fault==0){const int32_t index=1;std::memcpy(reinterpret_cast<uint8_t*>(&skeletal_first)+0x60c,&index,4);}
        else if(fault==1)skeletal_array(skeletal_first,0x5c8,{pose_render_b.data(),2,2});else if(fault==2)skeletal_array(skeletal_first,0x5c8,{pose_render_a.data(),1,2});
        else skeletal_array(skeletal_first,0x5c8,{pose_calc_a.data(),2,2});
        rejects([&]{pose_transfer(b,&result);},"aliased indices/storage or incomplete native pose buffers refuse without resizing");}
    pose_reset(reflect);b=binding();pose_callback_replace=true;rejects([&]{pose_transfer(b,&result);},"native notification callback cannot replace original render asset");
    pose_reset(reflect);b=binding();skeletal_array(skeletal_second,0x8b8,{pose_input.data(),1,2});
    rejects([&]{pose_transfer(b,&result);},"native calculator refresh refuses an incomplete local buffer before dispatch");check(pose_stages.empty(),"unproved local buffer never enters native refresh");
    pose_reset(reflect);b=binding();output=pose_transfer(b,&result);reinterpret_cast<uint8_t*>(&skeletal_first)[0x798]|=0x40;
    rejects([&]{pose_final(b,output);},"later component callback cannot leave a new unpublished pose dirty");
    pose_reset(reflect);b=binding();output=pose_transfer(b,&result);reinterpret_cast<uint8_t*>(&skeletal_first)[0x8a]|=8;
    rejects([&]{pose_final(b,output);},"later callback cannot activate the owned skeletal renderer");
    pose_reset(reflect);b=binding();output=pose_transfer(b,&result);skeletal_pointer(skeletal_first,0x568,UINT32_MAX);pose_final(b,output);
    check(pose_null_leader(0)&&pose_null_leader(UINT32_MAX)&&!pose_null_leader(uint64_t{1}<<32),"native ctor/reset NULL weak forms refuse a live leader serial");
    check((reinterpret_cast<uint8_t*>(&skeletal_first)[0xa42]&4)==0,"native suspended pose is admitted with original DisableClothSimulation false");
    reinterpret_cast<uint8_t*>(&skeletal_first)[0xa42]|=4;reinterpret_cast<uint8_t*>(&skeletal_first)[0xa52]&=static_cast<uint8_t>(~8u);
    bool exact_reason{};try{pose_pure(b);}catch(const Error& e){exact_reason=std::string(e.what())=="native pose cloth not suspended";}
    check(exact_reason,"DisableClothSimulation cannot substitute for suspension and refusal identifies the exact field");
    VertexSnapshot empty{};empty.kind=0;empty.suspended=8;empty.postprocess=1;vertex_skeletal_mirror_state(empty);
    check(empty.cloth==0,"native empty skeletal final guard accepts actual suspension without changing source DisableClothSimulation");
    empty.suspended=0;empty.cloth=4;rejects([&]{vertex_skeletal_mirror_state(empty);},"empty skeletal unrelated DisableClothSimulation bit never proves suspension");
    pose_build_admit=pose_shipping_build;pose_native_call=pose_native;pose_image=0;scene_vtable_read=scene_vtable;lifetime_reset(reflect);
}
}
int main() {
    try {
        hsmp_presentation_register(nullptr);check(installed==nullptr,"null provider registration");
        HsmpViewResult result{};check(inspect({}, {}, {}, nullptr, &result)==-1 && result.complete==0,"missing reflection refused");
        Transform pose{{1.25,-2.5,7},{0.1,0.2,0.3,0.9},{0.75,1.5,2}};
        auto native=engine(pose);check(native.p[0]==pose.p[0]&&native.q[0]==pose.q[0]&&native.scale[2]==pose.scale[2],"native transform field ordering");
        check(close(wire(native),pose),"native transform roundtrip");
        pose.q[0]=std::numeric_limits<double>::quiet_NaN();rejects([&]{engine(pose);},"NaN quaternion refused");
        HsmpViewComponent c{};c.relative={{0,0,0},{0,0,0,1},{1,1,1}};c.visible=1;
        c.kind=2;rejects([&]{recipe(c);},"groom cannot become ready");c.kind=0;
        c.vertex_state=3;rejects([&]{recipe(c);},"unavailable vertex state refused");c.vertex_state=1;
        rejects([&]{recipe(c);},"captured colors require actual LOD data");c.vertex_state=0;
        c.bone_count=513;rejects([&]{recipe(c);},"oversized bone dictionary refused");c.bone_count=0;
        HsmpViewFrame f{};f.bone_count=1;rejects([&]{frame(c,f);},"source frame dictionary mismatch refused");
        const wchar_t scene_path[]=L"/Script/Engine.SceneComponent";
        const wchar_t capsule_path[]=L"/Script/Engine.CapsuleComponent";
        const wchar_t unknown_path[]=L"/Script/Engine.LightComponent";
        const auto txt=[](const wchar_t* s)->HsmpViewText{return{u16(s),static_cast<uint32_t>(std::wcslen(s)),0};};
        HsmpViewComponent anchor{};anchor.id=3;anchor.kind=4;anchor.vertex_state=4;anchor.asset=txt(scene_path);anchor.relative={{0,0,0},{0,0,0,1},{1,1,1}};
        recipe(anchor);check(true,"proven nonrendering scene anchor has an explicit not-applicable vertex state");
        anchor.vertex_state=1;rejects([&]{recipe(anchor);},"scene anchor cannot carry captured colors");anchor.vertex_state=4;
        HsmpViewMaterial mat{};anchor.materials=&mat;anchor.material_count=1;rejects([&]{recipe(anchor);},"scene anchor cannot carry render materials");anchor.material_count=0;
        anchor.asset=txt(unknown_path);rejects([&]{recipe(anchor);},"unsupported scene class refuses");
        anchor.asset=txt(capsule_path);anchor.visible=1;rejects([&]{recipe(anchor);},"visible capsule cannot become a nonrendering anchor");
        anchor.visible=0;recipe(anchor);check(true,"effective-hidden exact capsule can be represented as an anchor");
        c.vertex_state=4;rejects([&]{recipe(c);},"render mesh cannot use a not-applicable vertex state");c.vertex_state=0;
        std::array<HsmpViewComponent,3> graph{};graph[0].id=1;graph[0].parent=2;graph[1].id=2;graph[1].parent=3;graph[2].id=3;touches=0;
        check(parent_order(graph.data(),3)==std::vector<uint32_t>({2,1,0}),"reversed parent input still applies every ancestor before its child");
        graph[2].parent=9;rejects([&]{parent_order(graph.data(),3);},"missing parent refuses before engine access");
        graph[2].parent=1;rejects([&]{parent_order(graph.data(),3);},"cyclic parent graph refuses before engine access");
        graph[2].parent=0;graph[2].id=1;rejects([&]{parent_order(graph.data(),3);},"duplicate scene node refuses before engine access");
        check(touches==0,"parent graph validation never touches an engine object");
        HsmpReflect reflect{};reflect.abi=HSMP_REFLECT_ABI;reflect.weak=weak;reflect.resolve=resolve;reflect.class_of=class_of;
        vt=&reflect;object_name=mock_name;game_thread=0;identities.clear();thread();
        auto object=keep(&obj);check(get(object)==&obj,"live weak identity");
        int32_t valid=1;HsmpViewGuard guard{&valid,guard_check};active_guard=&guard;
        check(get(object)==&obj,"borrowed guard accepts current binding");
        valid=0;touches=0;rejects([&]{get(object);},"changed binding refused between calls");
        check(touches==0,"guard refusal precedes all native object access");active_guard=nullptr;
        touches=0;check(inspect({}, {}, {}, &guard, &result)==-1&&touches==0,"provider entry refuses changed guard");
        check(active_guard==nullptr,"failed guard does not escape the borrowed call");
        obj.name=12;rejects([&]{get(object);},"serial-zero name reuse refused");
        rejects([&]{keep(&obj);},"reenrollment cannot overwrite old identity");obj.name=11;
        int32_t status{};touches=0;std::thread other([&]{HsmpViewResult out{};status=inspect({}, {}, {}, nullptr, &out);});other.join();
        check(status==-1&&touches==0,"wrong thread refuses before touching engine objects");
        mirrors.emplace(77,Mirror{{1,1},{2,2},{}});touches=0;discard(77);
        check(mirrors.empty()&&touches==0,"world discard never touches old UObject");
        mirrors.emplace(78,Mirror{{1,1},{2,2},{}});touches=0;destroy({3,3},78,nullptr);
        check(mirrors.empty()&&touches==0,"wrong-world destroy refuses before old actor access");
        lifetime_reset(reflect);auto world=keep(&old_world);auto mirror_actor=keep(&actor);valid=0;
        mirrors.emplace(90,Mirror{world,mirror_actor,{}});destroy(world,90,&guard);
        check(mirrors.empty()&&level_calls==0&&destroy_calls==0,"changed borrowed binding discards without ProcessEvent");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);valid=1;travel_on_get_level=true;
        mirrors.emplace(91,Mirror{world,mirror_actor,{}});destroy(world,91,&guard);
        check(level_calls==1&&destroy_calls==0&&mirrors.empty(),"GetLevel reentry with old world still alive discards without K2_DestroyActor");
        check(active_guard==nullptr&&!active_game_instance.weak,"failed destroy restores borrowed operation scope");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);
        mirrors.emplace(92,Mirror{world,mirror_actor,{}});destroy(world,92,&guard);
        check(level_calls==1&&destroy_calls==1&&mirrors.empty(),"current original world destroys its mirror exactly once");
        check(post_destroy_actor_touches==0&&invalid_actor_resolves==0,"destroyed mirror actor is never resolved or read after K2_DestroyActor");
        destroy(world,92,&guard);check(level_calls==1&&destroy_calls==1,"discarded mirror handle cannot destroy twice");
        check(provider.abi==11&&sizeof(provider)==112&&sizeof(HsmpViewFinishTarget)==128&&sizeof(HsmpViewVertexState)==24,"complete native empty-skeletal proof requires presentation ABI11");
        HsmpViewActorScope actor_scope_result{};
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);valid=1;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==1&&actor_scope_result.qualified==1
            &&actor_scope_result.state.listed==1&&(actor_scope_result.state.known&5)==5&&actor_scope_result.state.flags==0,
            "read-only scope preserves actual original native world membership and flags");
        check(level_calls==0&&destroy_calls==0&&array_frees==1,"actor scope invokes only static native census and frees its POD array");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);omit_world=true;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==1&&actor_scope_result.state.listed==0&&actor_scope_result.state.flags==0,
            "global memory-qualified actor absence is reported without guessed inactivity or removal");
        check(level_calls==0&&destroy_calls==0,"unlisted original receives no actor ProcessEvent");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);actor.flags=mirrored_garbage;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==1&&actor_scope_result.state.listed==0&&actor_scope_result.state.flags==mirrored_garbage,
            "memory-qualified engine garbage can be diagnosed without actor ProcessEvent");
        check(level_calls==0&&destroy_calls==0,"garbage actor receives no scope actor events");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);scope_garbage_during_census=true;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==1&&actor_scope_result.state.flags==mirrored_garbage,
            "scope flags are freshly read after static enumeration reentry");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);scope_reuse_during_census=true;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==-1&&actor_scope_result.qualified==0&&level_calls==0&&destroy_calls==0,
            "original name reuse during static enumeration cannot pass a stale metadata snapshot");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);scope_travel_during_census=true;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==-1&&actor_scope_result.qualified==0&&array_frees==1,
            "travel during scope enumeration refuses and frees the native output array");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);scope_disappear_during_census=true;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==-1&&actor_scope_result.qualified==0,
            "original disappearance during scope census stays unavailable rather than qualifying a new actor");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);retirement_flags=nullptr;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==-1&&actor_scope_result.qualified==0&&destroy_calls==0,
            "unknown native flag API cannot produce a qualified actor scope");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);valid=0;
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==-1&&actor_scope_result.qualified==0&&array_frees==0&&level_calls==0&&destroy_calls==0,
            "changed borrowed guard stops scope before native census or actor events");valid=1;
        HsmpViewRetirement retired{};
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);
        check(actor_scope(world,mirror_actor,&guard,&actor_scope_result)==1,"fixture obtains qualified original scope before retirement");
        const Obj mismatched_scope{mirror_actor.weak+1,mirror_actor.address};
        check(retire(world,mismatched_scope,0,{},&guard,&retired)==-1&&retired.qualified==0&&destroy_calls==0&&level_calls==0,
            "mismatched original weak cannot dispatch retirement against the address");
        actor.name=999;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0&&level_calls==0,
            "original name reuse between scope and retirement refuses before Actor ProcessEvent");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);actor.alive=false;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.qualified==0&&destroy_calls==0,"initially invalid weak actor cannot manufacture retirement proof");
        lifetime_reset(reflect);world=keep(&old_world);mirror_actor=keep(&actor);
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.qualified==0&&destroy_calls==0,"ordinary actor cannot enter driver retirement");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);actor_persistent=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0,"Persistent driver is never destroyed");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);retirement_owner=&foreign_owner;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.qualified==0&&destroy_calls==0,"driver with an actual foreign-world owner is never destroyed");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);travel_on_get_level=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0,"travel during driver qualification prevents native destroy");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);destroy_invalidates=false;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.qualified==1&&retired.dispatched==1&&retired.alive_after==1,"native dispatch with still-live weak identity refuses retirement");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);
        check(retire(world,mirror_actor,0,{},&guard,&retired)==1&&retired.qualified==1&&retired.dispatched==1&&retired.alive_after==0&&destroy_calls==1,"original live-world presence then world/global absence proves retirement");
        check(invalid_actor_resolves==0&&post_destroy_actor_touches==0,"post-retirement absence uses raw empty object-array slot without touching invalid actor memory");
        const auto retired_weak=retired.weak;
        check(probe_retirement(world,mirror_actor,&guard,&retired)==1&&retired.dispatched==0&&retired.alive_after==0&&destroy_calls==1,"fresh native probe confirms original retirement without another destroy");
        check(post_destroy_actor_touches==0,"native probe never dereferences a pending-garbage actor");
        actor.alive=true;actor.name=999;
        check(probe_retirement(world,mirror_actor,&guard,&retired)==-1&&destroy_calls==1,"same-address name reuse refuses without repeated destruction");
        actor.name=113;actor.alive=false;reuse_after_destroy=true;
        check(probe_retirement(world,mirror_actor,&guard,&retired)==-1&&destroy_calls==1,"retired weak slot address reuse refuses without touching replacement");
        reuse_after_destroy=false;touches=0;forget_retirements();
        check(retired_drivers.empty()&&touches==0,"world drop clears only scalar retirement proofs");
        check(probe_retirement(world,{retired_weak,mirror_actor.address},&guard,&retired)==-1&&retired.qualified==0,"world-dropped original proof cannot be reused");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);reuse_after_destroy=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.alive_after==2&&destroy_calls==1,"reused weak slot refuses and never destroys the replacement");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);travel_on_destroy=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.alive_after==2&&invalid_actor_resolves==0,"travel inside native destroy abandons post-call actor proof");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);destroy_invalidates=false;destroy_garbage=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==1&&retired.weak_present==1&&retired.before.listed==1&&retired.after.listed==0&&(retired.after.flags&mirrored_garbage),"UE4SS weak-valid plus native world absence and mirrored garbage proves engine retirement");
        check(post_destroy_actor_events==0,"garbage-but-allocated actor receives no post-dispatch ProcessEvent");
        check(probe_retirement(world,mirror_actor,&guard,&retired)==1&&destroy_calls==1&&post_destroy_actor_events==0,"fresh repeated probe preserves native garbage proof without actor events");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);destroy_invalidates=false;omit_after_destroy=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.after.listed==0&&retired.weak_present==1,"active-level-like absence without original garbage/global absence refuses");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);actor.flags=mirrored_garbage;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0,"already-garbage actor cannot manufacture prior live observation");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);omit_world=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0,"missing original live world observation refuses before destroy");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);retirement_flags=nullptr;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&destroy_calls==0,"missing native object flags API refuses");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);reuse_during_post_census=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.alive_after==2&&destroy_calls==1,"slot reused inside post-census ProcessEvent cannot pass a stale flags snapshot");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);destroy_invalidates=false;travel_during_post_census=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==-1&&retired.alive_after==2&&array_frees==2,"world change during post census refuses and frees both native arrays");
        lifetime_reset(reflect);actor.cls=&driver_class;world=keep(&old_world);mirror_actor=keep(&actor);destroy_invalidates=false;destroy_garbage=true;
        check(retire(world,mirror_actor,0,{},&guard,&retired)==1,"fixture obtains original garbage retirement proof");actor_memory_unqualified=true;
        check(probe_retirement(world,mirror_actor,&guard,&retired)==-1&&retired.alive_after==2,"weak-invalid but globally present object cannot masquerade as global disappearance");
        lifetime_reset(reflect);actor.cls=&driver_class;spline_flags=lifetime_flags;
        reflect.find=[](const uint16_t* key)->void*{if(std::wstring(reinterpret_cast<const wchar_t*>(key))==L"/Script/Engine.SplineComponent")return &driver_class;return lifetime_find(key);};
        const auto spline_owner_fixture=keep(&foreign_owner),spline_component_fixture=keep(&actor);
        actor.flags=mirrored_garbage;rejects([&]{SplineOperation op(spline_owner_fixture,spline_component_fixture);},"garbage original spline refuses before qualifier PE");check(level_calls==0,"no GetLevel dispatch on initially garbage component");
        actor.flags=0;foreign_owner.flags=mirrored_garbage;rejects([&]{SplineOperation op(spline_owner_fixture,spline_component_fixture);},"garbage original owner refuses before qualifier PE");foreign_owner.flags=0;
        {SplineOperation op(spline_owner_fixture,spline_component_fixture);Function spline_level(L"/Script/Engine.Actor:GetLevel");get_level_fn.flags=mirrored_garbage;
            rejects([&]{spline_level.call(spline_owner_fixture);},"garbage original function refuses before dispatch");check(level_calls==0,"no PE dispatched through garbage function");get_level_fn.flags=0;spline_garbage_after_call=true;
            rejects([&]{spline_level.call(spline_owner_fixture);},"native garbage callback invalidation refuses before continuation");check(level_calls==1,"callback invalidation has exactly one qualified dispatch");
            rejects([&]{spline_level.call(spline_owner_fixture);},"pending spline receives no repeated qualification PE");check(level_calls==1,"native weak-valid garbage never gets a second PE");}
        spline_garbage_after_call=false;spline_flags=nullptr;
        // Raw curve protocol tests retain original zero quaternion tangents and
        // engine POD alignment; they do not claim native rendering parity.
        HsmpViewSplineProfile profile{1,1,0,1,1};HsmpViewSplineFrame raw{};
        HsmpViewSplineVectorPoint vp{0,5,{1,2,3},{0,0,0},{4,5,6}};
        HsmpViewSplineQuatPoint qp{1,3,{2,3,4,5},{0,0,0,0},{7,8,9,10}};
        HsmpViewSplineFloatPoint fp{2,3,4,5,4};
        raw.position={&vp,1,1,-2.5f,0};raw.rotation={&qp,1,0,0,0};raw.reparam={&fp,1,0,1.25f,0};raw.settings.draw_debug=1;raw.version=99;
        spline_frame_valid(profile,raw);check(true,"raw nonunit quaternion and zero tangents are admitted without normalization");
        auto bad_profile=profile;bad_profile.metadata_null=0;rejects([&]{spline_frame_valid(bad_profile,raw);},"unknown spline metadata refuses");
        bad_profile=profile;bad_profile.position_count=65;rejects([&]{spline_frame_valid(bad_profile,raw);},"spline profile cap refuses before allocation");
        auto bad_frame=raw;bad_frame.position.count=0;rejects([&]{spline_frame_valid(profile,bad_frame);},"changed raw array count requires recipe revision");
        bad_frame=raw;bad_frame.settings.closed_loop=2;rejects([&]{spline_frame_valid(profile,bad_frame);},"unknown native settings bool refuses");
        qp.interp=6;rejects([&]{spline_frame_valid(profile,raw);},"unknown native interpolation mode refuses");qp.interp=3;
        fp.arrive=std::numeric_limits<float>::infinity();rejects([&]{spline_frame_valid(profile,raw);},"nonfinite native reparam tangent refuses");fp.arrive=4;
        spline_api.allocate=test_spline_allocate;spline_api.release=test_spline_release;
        spline_allocations=spline_releases=0;
        {SplineOwnedArray owned;spline_allocate<NativeQuatPoint>(owned,raw.rotation);auto* point=static_cast<NativeQuatPoint*>(owned.data);
            check(reinterpret_cast<uintptr_t>(point)%16==0&&owned.bytes==128,"engine quaternion POD buffer has native size/alignment");
            check(point->out[0]==2&&point->arrive[3]==0&&point->leave[3]==10&&point->interp==3,"raw quaternion/tangents copied exactly");
            check(qp.out[0]==2&&qp.arrive[3]==0,"destination allocation never edits source raw points");}
        check(spline_allocations==1&&spline_releases==1,"uncommitted engine array freed exactly once");
        spline_allocations=spline_releases=0;spline_fail_after=1;
        rejects([&]{SplineOwnedArray first,second;spline_allocate<NativeVectorPoint>(first,raw.position);spline_allocate<NativeQuatPoint>(second,raw.rotation);},"partial native allocation failure refuses");
        check(spline_allocations==2&&spline_releases==1,"partial allocation failure frees every allocated array");spline_fail_after=-1;
        alignas(16) NativeQuatPoint spline_native{};spline_native.key=1;spline_native.out[0]=2;spline_native.arrive[3]=0;spline_native.leave[3]=10;spline_native.interp=3;
        std::array<uint8_t,24> header{};Array array{&spline_native,1,1};std::memcpy(header.data(),&array,16);header[16]=1;float offset=-1.75f;std::memcpy(header.data()+20,&offset,4);
        HsmpViewSplineQuatCurve curve{};std::vector<HsmpViewSplineQuatPoint> decoded;
        spline_read_curve<NativeQuatPoint>(header.data(),curve,decoded,64);
        check(curve.looped==1&&curve.loop_key_offset==-1.75f&&decoded[0].out[0]==2&&decoded[0].arrive[3]==0,"checked POD raw read preserves loop and zero tangent");
        array.capacity=0;std::memcpy(header.data(),&array,16);rejects([&]{spline_read_curve<NativeQuatPoint>(header.data(),curve,decoded,64);},"count exceeding native capacity refuses");
        array={nullptr,1,1};std::memcpy(header.data(),&array,16);rejects([&]{spline_read_curve<NativeQuatPoint>(header.data(),curve,decoded,64);},"nonnull count with missing POD data refuses");
        HsmpViewSplineProfile empty{0,0,0,0,1};HsmpViewSplineFrame empty_frame{};spline_frame_valid(empty,empty_frame);check(true,"actual empty spline curves are preserved without invented points");
        SplineSnapshot a{},b{};a.value.version=b.value.version=7;check(spline_equal(a,b),"two identical empty raw copies agree");b.value.version=8;check(!spline_equal(a,b),"source curve version mutation invalidates coherent capture");b=a;b.value.settings.duration=2;check(!spline_equal(a,b),"source settings mutation invalidates coherent capture");
        path_checks(reflect);
        vertex_checks(reflect);
        scene_checks(reflect);
        empty_checks(reflect);
        skeletal_checks(reflect);
        pose_checks(reflect);
        create_trace_checks(reflect);
        check(profile_ffi_calls==0,"ordinary capture/guard/lifetime paths make no profile FFI calls");
        {StaticProfileTraceScope trace;profile_tick(0);profile_phase("fixture_profile",0);}
        const auto trace_calls=profile_ffi_calls;profile_tick(0);profile_phase("inactive",0);
        check(trace_calls==2&&profile_ffi_calls==trace_calls&&!static_profile_trace,"only static describe trace scope enables FFI then restores inactivity");
        std::cout<<checks<<" native presentation lifetime/rejection checks passed\n";return 0;
    }catch(const std::exception& e){std::cerr<<e.what()<<'\n';return 1;}
}
