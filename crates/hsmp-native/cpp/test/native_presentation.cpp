// Rejection/lifetime checks for the production provider; no mocked native parity claim.
#include "../src/native_presentation.cpp"
#include <iostream>
#include <limits>
#include <thread>
static const HsmpPresentation* installed{};
extern "C" void hsmp_native_set_presentation(const HsmpPresentation* p) {installed=p;}
namespace {
int checks{}, touches{};
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
struct LifetimeObject {uint64_t name{};LifetimeObject* cls{};LifetimeObject* property{};bool alive{true};};
LifetimeObject meta{100},world_class{101},gi_class{102},actor_class{103},level_class{104},function_class{105};
LifetimeObject old_world{110,&world_class},new_world{111,&world_class},gi{112,&gi_class};
LifetimeObject actor{113,&actor_class},level{114,&level_class};
LifetimeObject get_level_fn{115,&function_class},destroy_fn{116,&function_class};
LifetimeObject driver_class{117,&meta},tag_fn{118,&function_class},owner_fn{119,&function_class},replacement{120,&driver_class};
LifetimeObject foreign_owner{121,&actor_class},foreign_level{122,&level_class};
LifetimeObject* retirement_owner{};
std::vector<LifetimeObject*> lifetime_objects;
std::map<std::wstring,uint64_t> lifetime_names;
LifetimeObject* current_world{};
bool travel_on_get_level{};
bool actor_persistent{},destroy_invalidates{true},reuse_after_destroy{},travel_on_destroy{};
int level_calls{},destroy_calls{},post_destroy_actor_touches{},invalid_actor_resolves{};
void lifetime_touch(LifetimeObject* o) {if(o==&actor&&!actor.alive)++post_destroy_actor_touches;}
uint64_t lifetime_weak(void* p) {for(size_t i=0;i<lifetime_objects.size();++i)if(lifetime_objects[i]==p)return i+1;return 0;}
void* lifetime_resolve(uint64_t id) {
    if(id==0||id>lifetime_objects.size())return nullptr;
    auto o=lifetime_objects[static_cast<size_t>(id-1)];
    if(o==&actor&&destroy_calls&&reuse_after_destroy)return &replacement;
    if(o==&actor&&!actor.alive)++invalid_actor_resolves;return o->alive?o:nullptr;
}
void* lifetime_class(void* p) {auto o=static_cast<LifetimeObject*>(p);lifetime_touch(o);return o->cls;}
const uint64_t* lifetime_name(const void* p) {auto o=const_cast<LifetimeObject*>(static_cast<const LifetimeObject*>(p));lifetime_touch(o);return &o->name;}
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
    return -1;
}
int32_t lifetime_prop(void* object,const uint16_t* key,HsmpProp* out) {
    const std::wstring field(reinterpret_cast<const wchar_t*>(key));
    if((object==&old_world||object==&new_world)&&field==L"OwningGameInstance") {*out=object_field(L"OwningGameInstance",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    if(object==&level&&field==L"OwningWorld") {*out=object_field(L"OwningWorld",static_cast<int32_t>(offsetof(LifetimeObject,property)));return 1;}
    return 0;
}
void lifetime_call(void* object,void* fn,void* params) {
    check(object==&actor||(object==&foreign_owner&&fn==&get_level_fn),"destroy path invokes only its original actor or qualifies its owner");
    if(fn==&get_level_fn){++level_calls;auto result=object==&foreign_owner?&foreign_level:&level;std::memcpy(params,&result,sizeof(result));if(travel_on_get_level)current_world=&new_world;}
    else if(fn==&destroy_fn){++destroy_calls;if(destroy_invalidates)actor.alive=false;if(travel_on_destroy)current_world=&new_world;}
    else if(fn==&tag_fn){static_cast<uint8_t*>(params)[8]=actor_persistent?1:0;}
    else if(fn==&owner_fn){std::memcpy(params,&retirement_owner,sizeof(retirement_owner));}
    else throw std::runtime_error("unexpected lifetime function");
}
void* lifetime_world(const void* object) {return object==&gi?current_world:nullptr;}
void lifetime_reset(HsmpReflect& reflect) {
    for(auto c:{&meta,&world_class,&gi_class,&actor_class,&level_class,&function_class})c->cls=&meta;
    lifetime_objects={&meta,&world_class,&gi_class,&actor_class,&level_class,&function_class,&old_world,&new_world,&gi,&actor,&level,&get_level_fn,&destroy_fn,&driver_class,&tag_fn,&owner_fn,&replacement,&foreign_owner,&foreign_level};
    for(auto o:lifetime_objects)o->alive=true;
    old_world.property=&gi;new_world.property=&gi;level.property=&old_world;current_world=&old_world;
    foreign_level.property=&new_world;retirement_owner=nullptr;
    actor.cls=&actor_class;actor.name=113;
    travel_on_get_level=false;level_calls=0;destroy_calls=0;post_destroy_actor_touches=0;invalid_actor_resolves=0;
    actor_persistent=false;destroy_invalidates=true;reuse_after_destroy=false;travel_on_destroy=false;
    identities.clear();names.clear();signatures.clear();lifetime_names.clear();mirrors.clear();
    retired_drivers.clear();
    active_guard=nullptr;active_world={};active_game_instance={};
    reflect.fname=lifetime_fname;reflect.find=lifetime_find;reflect.is_a=lifetime_is_a;reflect.class_of=lifetime_class;
    reflect.props=lifetime_props;reflect.obj_prop=lifetime_prop;reflect.call=lifetime_call;
    reflect.weak=lifetime_weak;reflect.resolve=lifetime_resolve;object_name=lifetime_name;object_world=lifetime_world;
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
        check(provider.abi==4,"guarded native retirement requires presentation ABI4");
        HsmpViewRetirement retired{};
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
        check(retire(world,mirror_actor,0,{},&guard,&retired)==1&&retired.qualified==1&&retired.dispatched==1&&retired.alive_after==0&&destroy_calls==1,"only originally qualified native weak invalidity proves retirement");
        check(invalid_actor_resolves==1&&post_destroy_actor_touches==0,"post-retirement proof reads the object-array slot without touching invalid actor memory");
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
        std::cout<<checks<<" native presentation lifetime/rejection checks passed\n";return 0;
    }catch(const std::exception& e){std::cerr<<e.what()<<'\n';return 1;}
}
