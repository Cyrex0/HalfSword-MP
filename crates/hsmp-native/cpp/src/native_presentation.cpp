// Inert render replicas. All reflection is synchronous and game-thread guarded by Rust.
// Layouts are checked against reflected properties before any native memory read.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include "native_presentation.h"
#include "hsmp_native.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <map>
#include <memory>
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
using SourceOuter=const void* const*(*)(const void*);
SourceOuter source_outer{};
struct Identity {uint64_t address{},name{},class_weak{},class_address{};};
std::map<uint64_t,Identity> identities;
struct Error : std::runtime_error { using std::runtime_error::runtime_error; };
void require(bool ok, const char* why) { if (!ok) throw Error(why); }
thread_local const HsmpViewGuard* active_guard{};
thread_local Obj active_world{},active_game_instance{};
void mesh_call_guard();
struct LookupEntry {std::vector<HsmpNativePathNode> original,pinned;std::vector<uint32_t> flags,class_flags;uint64_t package{};void* zero_item{};};
// Copied expectations only: no successful validation survives a pure boundary.
struct LookupObjectWitness {HsmpNativePathNode node{};uint64_t outer{};uint32_t flags{};void* zero_item{};};
struct LookupClassWitness {Obj object{};uint64_t name{};uint32_t flags{};bool exact_serial{};};
struct LookupWitnesses {std::map<uint64_t,LookupObjectWitness> objects;std::map<uint64_t,LookupClassWitness> classes;std::optional<uint64_t> package;};
struct LookupState {Obj world{},gi{};HsmpNativePathNode world_node{},gi_node{};HsmpProp gi_property{};std::map<std::wstring,LookupEntry> entries;LookupWitnesses witnesses;};
thread_local LookupState* active_lookup{};
struct CaptureWatch;
thread_local const CaptureWatch* active_capture_watch{};
Obj lookup_find(const wchar_t* path);
void lookup_finish();
void lookup_start(LookupState& state,Obj world,Obj gi);
void capture_watch_finish();
void capture_watch_links();
void lookup_trace_begin();
void lookup_trace_end(bool complete);
void lookup_trace_request(const wchar_t* path,uint32_t category);
void lookup_trace_native(const wchar_t* path,uint64_t elapsed_us);
void lookup_trace_flush();
bool mesh_serial_assignment(Obj original,Obj current);
thread_local bool static_profile_trace{};
thread_local HsmpNativeCaptureRow* active_capture_trace{};
uint64_t* present_capture_nanoseconds(uint32_t index);
void timer_accumulate(uint64_t& total,uint64_t nanoseconds,bool precise){
    if(precise&&nanoseconds>~uint64_t{}-total)total=~uint64_t{};
    else total+=precise?nanoseconds:nanoseconds/1000;
}
struct LookupNativeTrace {const wchar_t* path;HsmpNativeCaptureRow* row;uint64_t before;
    explicit LookupNativeTrace(const wchar_t* key):path(key),row(active_capture_trace),before(row?row->us[6]:0){}
    ~LookupNativeTrace(){lookup_trace_native(path,row?row->us[6]-before:0);}
};
struct CaptureTimer {
    uint64_t* target{};bool precise{};std::chrono::steady_clock::time_point start{};
    explicit CaptureTimer(uint32_t index,bool enabled=true){if(enabled&&active_capture_trace){target=present_capture_nanoseconds(index);precise=target!=nullptr;
        if(!target)target=&active_capture_trace->us[index];start=std::chrono::steady_clock::now();}}
    ~CaptureTimer(){if(target)timer_accumulate(*target,static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now()-start).count()),precise);}
};
struct CaptureTrace {
    HsmpNativeCaptureRow row{};HsmpNativeCaptureRow* previous{active_capture_trace};bool enabled{hsmp_native_capture_profile_active()==1};
    std::chrono::steady_clock::time_point start{},boundary{};
    explicit CaptureTrace(bool allowed=true):enabled(allowed&&hsmp_native_capture_profile_active()==1){if(enabled){lookup_trace_begin();start=boundary=std::chrono::steady_clock::now();active_capture_trace=&row;}else if(allowed)lookup_trace_end(true);}
    void label(const HsmpViewComponent& c){row.component=c.id;row.kind=c.kind;}
    void mark(uint32_t stage){if(enabled){const auto now=std::chrono::steady_clock::now();row.us[stage]=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(now-boundary).count());boundary=now;}}
    ~CaptureTrace(){if(enabled){row.us[0]=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-start).count());active_capture_trace=previous;hsmp_native_capture_profile_row(&row);lookup_trace_end(row.complete==1);}}
};
struct StaticProfileTraceScope {bool previous;StaticProfileTraceScope():previous(static_profile_trace){static_profile_trace=true;}~StaticProfileTraceScope(){static_profile_trace=previous;}};
void profile_phase(const char* stage,uint32_t edge){if(static_profile_trace)hsmp_native_profile_checkpoint(stage,edge);}
void profile_tick(uint32_t counter){if(active_capture_trace){if(counter==0)++active_capture_trace->guards;else if(counter==2)++active_capture_trace->finds;else if(counter==3)++active_capture_trace->events;}if(static_profile_trace)hsmp_native_profile_tick(counter);}
std::atomic<HsmpPresentationCreateLog> create_logger{};
thread_local bool present_provider_active{};
thread_local uint32_t present_apply_profile_attempts{},present_finish_profile_attempts{};
thread_local bool present_finish_pending{};
struct PresentExtraCounters {std::array<uint64_t,3> ns{},count{};std::array<uint64_t,8> capture_ns{};HsmpNativeCaptureRow* row{};};
thread_local PresentExtraCounters* present_extra{};
uint64_t* present_capture_nanoseconds(uint32_t index){return present_provider_active&&present_extra&&present_extra->row==active_capture_trace?&present_extra->capture_ns[index]:nullptr;}
struct PresentExtraTimer {
    PresentExtraCounters* owner{};uint32_t index{};bool enabled{};
    std::chrono::steady_clock::time_point start{};
    explicit PresentExtraTimer(uint32_t bucket,bool activate=true):owner(present_provider_active&&present_extra&&present_extra->row==active_capture_trace?present_extra:nullptr),index(bucket){
        if(owner){start=std::chrono::steady_clock::now();if(activate)enable();}
    }
    void enable(){if(owner&&!enabled){enabled=true;++owner->count[index];}}
    ~PresentExtraTimer(){if(enabled)timer_accumulate(owner->ns[index],static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now()-start).count()),true);}
};
struct PresentProviderTrace {
    HsmpNativeCaptureRow row{};HsmpNativeCaptureRow* previous{};
    PresentExtraCounters extra{};PresentExtraCounters* previous_extra{};
    bool enabled{},complete{},aggregate{};uint32_t attempt{};
    std::chrono::steady_clock::time_point start{};
    explicit PresentProviderTrace(bool finish=false):aggregate(finish){}
    void begin(bool warm){
        if(!warm||active_capture_trace||present_provider_active||!create_logger.load())return;
        auto& attempts=aggregate?present_finish_profile_attempts:present_apply_profile_attempts;
        if(attempts>=2||(aggregate&&!present_finish_pending))return;
        attempt=++attempts;enabled=true;start=std::chrono::steady_clock::now();previous=active_capture_trace;
        active_capture_trace=&row;extra.row=&row;previous_extra=present_extra;present_extra=&extra;present_provider_active=true;
        if(aggregate)present_finish_pending=false;else present_finish_pending=true;
    }
    ~PresentProviderTrace(){
        if(!enabled)return;
        row.us[0]=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-start).count());
        for(uint32_t bucket=1;bucket<8;++bucket)row.us[bucket]=extra.capture_ns[bucket]/1000;
        active_capture_trace=previous;present_extra=previous_extra;present_provider_active=false;
        // Declared before provider locks/scopes: only copied scalars remain here.
        if(const auto logger=create_logger.load()){
            const auto label=aggregate?"finish":"apply";
            for(uint32_t bucket=0;bucket<8;++bucket)logger(aggregate?"present_finish_us":"present_apply_us",complete?1u:0u,row.us[bucket],attempt,bucket,0,0,label);
            logger(aggregate?"present_finish_count":"present_apply_count",complete?1u:0u,row.guards,attempt,0,0,0,label);
            logger(aggregate?"present_finish_count":"present_apply_count",complete?1u:0u,row.finds,attempt,1,0,0,label);
            logger(aggregate?"present_finish_count":"present_apply_count",complete?1u:0u,row.events,attempt,2,0,0,label);
            for(uint32_t bucket=0;bucket<3;++bucket){logger("present_extra_us",complete?1u:0u,extra.ns[bucket]/1000,attempt,bucket,0,0,label);
                logger("present_extra_count",complete?1u:0u,extra.count[bucket],attempt,bucket,0,0,label);}
        }
    }
};
struct LookupTraceCount {uint64_t raw{},us{};uint32_t cold{},hit{},bootstrap{},capacity{};};
struct LookupTraceReport {bool attempted{},collecting{};uint32_t rows{},truncated{},pending{};LookupTraceCount total{};std::map<std::wstring,LookupTraceCount> paths;};
thread_local LookupTraceReport lookup_trace_report;
void lookup_trace_begin(){auto& r=lookup_trace_report;if(!r.attempted&&create_logger.load()){r.attempted=true;r.collecting=true;}}
LookupTraceCount* lookup_trace_path(const wchar_t* path)noexcept{auto& r=lookup_trace_report;if(!r.collecting)return nullptr;try{
    const auto found=r.paths.find(path);if(found!=r.paths.end())return &found->second;
    if(r.paths.size()>=64){++r.truncated;return nullptr;}return &r.paths.emplace(path,LookupTraceCount{}).first->second;}catch(...){++r.truncated;return nullptr;}}
void lookup_trace_request(const wchar_t* path,uint32_t category){if(!active_capture_trace||!lookup_trace_report.collecting)return;
    auto add=[category](LookupTraceCount& c){if(category==0)++c.hit;else if(category==1)++c.cold;else if(category==2)++c.bootstrap;else if(category==3)++c.capacity;};
    add(lookup_trace_report.total);if(auto* c=lookup_trace_path(path))add(*c);}
void lookup_trace_native(const wchar_t* path,uint64_t elapsed_us){if(!active_capture_trace||!lookup_trace_report.collecting)return;
    ++lookup_trace_report.total.raw;lookup_trace_report.total.us+=elapsed_us;if(auto* c=lookup_trace_path(path)){++c->raw;c->us+=elapsed_us;}}
uint32_t lookup_trace_hash(const std::wstring& path){uint32_t hash=2166136261u;for(const auto ch:path){hash^=static_cast<uint16_t>(ch);hash*=16777619u;}return hash;}
void lookup_trace_emit(uint32_t status){auto report=std::move(lookup_trace_report);lookup_trace_report=LookupTraceReport{};lookup_trace_report.attempted=true;
    const auto logger=create_logger.load();if(!logger)return;
    const auto emit=[logger](const char* label,uint32_t hash,const LookupTraceCount& c){logger("capture_lookup_counts",c.bootstrap,c.raw,c.cold,c.hit,c.capacity,hash,label);
        logger("capture_lookup_time",0,c.us,0,0,0,hash,label);};
    for(const auto& [path,c]:report.paths){char label[64]{};const auto split=path.find_last_of(L'.');const auto first=path.size()<sizeof(label)?0:split==std::wstring::npos?0:split+1;
        const auto length=std::min(path.size()-first,sizeof(label)-1);for(size_t i=0;i<length;++i){const auto ch=path[first+i];label[i]=ch>=32&&ch<127&&ch!='"'&&ch!='\\'?static_cast<char>(ch):'?';}
        emit(label,lookup_trace_hash(path),c);}
    emit("__first_frame__",0,report.total);
    logger("capture_lookup_summary",status,report.rows,static_cast<uint32_t>(report.paths.size()),report.truncated,0,0,"__first_frame__");
}
void lookup_trace_end(bool complete){if(!lookup_trace_report.collecting){lookup_trace_flush();return;}
    if(!active_capture_trace){if(!complete){++lookup_trace_report.rows;lookup_trace_report.pending=2;}
        else if(hsmp_native_capture_profile_active()==1){if(++lookup_trace_report.rows>=98)lookup_trace_report.pending=1;}
        else lookup_trace_report.pending=3;
        if(lookup_trace_report.pending){lookup_trace_report.collecting=false;lookup_trace_flush();}}
}
void lookup_trace_flush(){if(!active_lookup&&!active_capture_trace&&lookup_trace_report.pending)lookup_trace_emit(lookup_trace_report.pending);}
std::atomic<uint64_t> create_operation{};
struct CreateTrace;
thread_local CreateTrace* active_create_trace{};
struct CreateTrace {
    HsmpPresentationCreateLog logger{create_logger.load()};
    CreateTrace* previous{active_create_trace};
    uint64_t operation{};uint32_t markers{},component{},kind{UINT32_MAX},pe_calls{},last_function{};
    char last_name[64]{"none"};bool limited{},pe_limited{},finished{};
    explicit CreateTrace(){active_create_trace=logger?this:nullptr;if(logger){operation=create_operation.fetch_add(1)+1;emit("create",0);}}
    ~CreateTrace(){active_create_trace=previous;}
    void emit(const char* stage,uint32_t edge,uint32_t function_id=0,const char* function_name="none") {
        if(!logger||finished||limited)return;
        // Reserve one explicit exhaustion marker and one terminal marker.
        if(markers>=126){limited=true;logger("trace_limit",3,operation,++markers,component,kind,function_id,function_name);return;}
        logger(stage,edge,operation,++markers,component,kind,function_id,function_name);
    }
    void part(uint32_t id,uint32_t value){component=id;kind=value;last_function=0;std::memcpy(last_name,"none",5);emit("component",0);}
    bool pe_enter(uint32_t function_id,const char* function_name){
        last_function=function_id;const auto length=std::min(std::strlen(function_name),sizeof(last_name)-1);
        std::memcpy(last_name,function_name,length);last_name[length]='\0';
        if(!logger||finished||limited)return false;
        if(pe_calls>=32){if(!pe_limited){pe_limited=true;emit("pe_limit",3);}return false;}
        if(markers>124){emit("trace_limit",3,function_id,function_name);limited=true;return false;}
        ++pe_calls;emit("pe",0,function_id,function_name);return true;
    }
    void terminal(uint32_t edge){if(logger&&!finished){logger("create",edge,operation,++markers,component,kind,last_function,last_name);finished=true;}}
};
// Hash and printable identifier use only the fixed reflected function path,
// never a UObject name, address, recipe value or FName conversion.
uint32_t create_function_id(const wchar_t* path){uint32_t id=2166136261u;for(;*path;++path){id^=static_cast<uint16_t>(*path);id*=16777619u;}return id;}
void create_function_name(const wchar_t* path,char* out,size_t capacity){
    const auto colon=std::wcsrchr(path,L':');const auto start=colon?colon+1:path;size_t i{};
    for(;start[i]&&i+1<capacity;++i){const auto c=start[i];if(!((c>=L'A'&&c<=L'Z')||(c>=L'a'&&c<=L'z')||(c>=L'0'&&c<=L'9')||c==L'_'))break;out[i]=static_cast<char>(c);}
    if(start[i]){std::memcpy(out,"other",6);return;}out[i]='\0';
}
void check_guard() {
    CaptureTimer capture_guard_time(5);
    if(!active_guard){mesh_call_guard();return;}
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
    mesh_call_guard();
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
    check_guard();const auto value=vt->fname(u16(s),1);check_guard();names.emplace(s,value);return value;
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
    PresentExtraTimer present_identity_time(1);
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
Obj find(const wchar_t* path) {return lookup_find(path);}
bool is(Obj o, const wchar_t* cls) { const auto c = find(cls); return vt->is_a(get(o), get(c)) != 0; }
Obj asset(HsmpViewText path, const wchar_t* cls) {
    const auto p = text(path);
    require(!p.empty() && p[0] == L'/', "native asset path missing");
    Obj o = find(p.c_str()); require(is(o, cls), "native asset class mismatch"); return o;
}
bool same(Obj a, Obj b) { return a.weak == b.weak && a.address == b.address; }
HsmpProp property(Obj o, const wchar_t* key, const wchar_t* type, int size) {
    HsmpProp p{};
    void* object=get(o);int32_t found{};
    {PresentExtraTimer present_property_time(0);found=vt->obj_prop(object,u16(key),&p);}
    require(found==1, "native object property missing");
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
    LookupState lookup{};LookupState* previous_lookup{};
    const CaptureWatch* previous_capture_watch{};
    explicit OperationScope(const HsmpViewGuard* guard,Obj world):previous(active_guard),previous_world(active_world),previous_gi(active_game_instance),previous_lookup(active_lookup),previous_capture_watch(active_capture_watch) {
        require(guard && guard->context && guard->check && guard->check(guard->context)==1,"native borrowed guard missing");
        active_guard=guard;active_world={};active_game_instance={};active_lookup=nullptr;active_capture_watch=nullptr;
        try {
            get(world);require(is(world,L"/Script/Engine.World"),"native guard world class");
            auto gi=object_property(world,L"OwningGameInstance");require(gi.weak&&is(gi,L"/Script/Engine.GameInstance"),"native owning game-instance missing");
            active_world=world;active_game_instance=gi;check_guard();lookup_start(lookup,world,gi);active_lookup=&lookup;
        }catch(...) {active_guard=previous;active_world=previous_world;active_game_instance=previous_gi;active_lookup=previous_lookup;active_capture_watch=previous_capture_watch;throw;}
    }
    ~OperationScope(){active_guard=previous;active_world=previous_world;active_game_instance=previous_gi;active_lookup=previous_lookup;active_capture_watch=previous_capture_watch;}
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
    uint32_t create_id{};char create_name[64]{};
    Function(const wchar_t* path) {
        PresentExtraTimer present_signature_time(2,false);
        if(active_create_trace){create_id=create_function_id(path);create_function_name(path,create_name,sizeof(create_name));}
        auto existing=signatures.find(path);
        if(existing!=signatures.end()) {
            present_signature_time.enable();
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
        lookup_finish();
        profile_tick(3);profile_phase("cpp_pe",0);
        auto* trace=active_create_trace;
        const bool traced=trace&&trace->pe_enter(create_id,create_name);
        {CaptureTimer capture_pe_time(7);vt->call(object_pointer,function_pointer,buf.data());}
        if(traced)trace->emit("pe",1,create_id,create_name);
        profile_phase("cpp_pe",1);
        check_guard();
        spline_call_guard(object,function,cls);
        vertex_call_guard(object,function,cls);
        scene_call_guard(object,function,cls);
        get(object); get(function); get(cls); if (result) ++result->operations;
        capture_watch_finish();
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
void mesh_assignment_error(Obj actual,Obj expected,HsmpViewText expected_path,bool calculator,const char* stage){
    // The path is copied from the already validated recipe. No UObject name or
    // class conversion, pointer formatting or new native query is needed here.
    const auto path=text(expected_path);const auto separator=path.find_last_of(L"./");const auto begin=separator==std::wstring::npos?0:separator+1;
    char label[33]{};const auto count=std::min(path.size()-begin,sizeof(label)-1);
    for(size_t i=0;i<count;++i){const auto ch=path[begin+i];label[i]=ch>=32&&ch<127&&ch!='"'&&ch!='\\'?static_cast<char>(ch):'?';}
    const bool address_equal=actual.address==expected.address,index_equal=static_cast<uint32_t>(actual.weak)==static_cast<uint32_t>(expected.weak);
    const bool zero_to_nonzero=(expected.weak>>32)==0&&(actual.weak>>32)!=0;
    char reason[192]{};std::snprintf(reason,sizeof(reason),"mirror %s mesh assignment failed; stage=%s; actual=%s; class=SkeletalMesh; address_equal=%u; index_equal=%u; zero_to_nonzero=%u; asset=%s",
        calculator?"pose calculator":"render",stage,actual.weak?"different":"absent",address_equal?1u:0u,index_equal?1u:0u,zero_to_nonzero?1u:0u,label);throw Error(reason);
}
void mesh_assignment(Obj component,Obj expected,HsmpViewText expected_path,bool calculator,const char* stage,HsmpViewResult* r){
    const auto actual=mesh_asset(component,r);if(!same(actual,expected))mesh_assignment_error(actual,expected,expected_path,calculator,stage);
}
struct MeshBinding {
    Obj original{},pinned{},world{},owner{};
    HsmpNativePathNode world_node{},owner_node{},level_node{};
    std::array<HsmpNativePathNode,64> path{};uint32_t count{},flags{};uint64_t package{};
};
MeshBinding mesh_binding(Obj world,Obj owner,HsmpViewText path,HsmpViewResult* r);
void mesh_binding_final(const MeshBinding& binding);
void mesh_bindings_final(const std::vector<const MeshBinding*>& bindings);
struct MeshPlan;
struct MeshWatch;
std::shared_ptr<const MeshPlan> mesh_watch_plan(const MeshWatch* watch);
void mesh_plan_final(const MeshPlan& plan);
Obj mesh_admit(MeshBinding& binding,Obj component,HsmpViewText path,bool calculator,const char* stage,HsmpViewResult* r);
bool effective_visible(Obj component) {return bool_property(component,L"bVisible")&&!bool_property(component,L"bHiddenInGame");}
void visibility(Obj component,bool visible,HsmpViewResult* r);
void collision_off(Obj component,HsmpViewResult* r);
#include "native_spline_impl.h"
#include "native_vertex_state_impl.h"
bool close(const Transform&,const Transform&);
#include "native_scene_impl.h"
#include "native_pose_impl.h"
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
void supported(Obj world,Obj owner,Obj component,HsmpViewResult* r,bool empty_skeletal=false) {
    qualify(world,owner,component,r);
    if (is(component,L"/Script/Engine.SkinnedMeshComponent")) {
        if(empty_skeletal){vertex_empty_profile(component,0);const auto proof=vertex_observe(world,owner,component,r);
            require(proof.component_kind==0&&proof.asset_present==0&&proof.no_override==1,"native empty skeletal state unsupported");
            require(!object_property(component,L"MeshDeformer").weak&&!object_property(component,L"PhysicsAssetOverride").weak&&!bool_property(component,L"bSetMeshDeformer"),"native empty skeletal deformer/physics override unsupported");
            require(!returned(component,L"/Script/Engine.SkinnedMeshComponent:GetMeshDeformerInstance",r).weak,"native empty skeletal deformer instance unsupported");
            qualify(world,owner,component,r);return;}
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
    require((c.kind<=1||(c.kind>=4&&c.kind<=9)) && c.visible<=1 && pointers(c.bones,c.bone_count,512) && pointers(c.morphs,c.morph_count,128) &&
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
    }else if(c.kind==8||c.kind==9){require(text(c.asset)==(c.kind==9?L"/Script/Engine.SkeletalMeshComponent":L"/Script/Engine.StaticMeshComponent")&&text(c.skeleton).empty()&&c.vertex_state==4&&!c.vertex_count,"native empty mesh recipe");}
    else require(c.vertex_state==0 || c.vertex_state==1,"native vertex recipe incomplete");
    if(c.kind!=5)require(c.spline.position_count==0&&c.spline.rotation_count==0&&c.spline.scale_count==0&&c.spline.reparam_count==0&&c.spline.metadata_null==0,"native non-spline profile present");
    require((c.kind==7)==(c.spring_arm_socket.len>0),"native spring arm singleton recipe presence");
    if(c.kind==7){const auto s=text(c.spring_arm_socket);require(s.size()<=128&&s!=L"None","native spring arm socket recipe");}
    require(c.vertex_state!=1 || c.vertex_count>0,"native vertex colors missing");
    require(c.kind==0 || (!c.bone_count&&!c.morph_count&&!c.hidden_count),"static component skeletal dictionary");
    engine(c.relative); for(uint32_t i=0;i<c.material_count;++i) {
        const auto& m=c.materials[i]; require(m.slot<32 && pointers(m.scalars,m.scalar_count,128) &&
            pointers(m.vectors,m.vector_count,128) && pointers(m.textures,m.texture_count,128),"native material dictionary bounds");
        if(text(m.base).empty())require(c.kind==9&&!m.scalar_count&&!m.vector_count&&!m.texture_count,"native null material unsupported outside empty skeletal");
        if(c.kind==9)require(m.slot==i,"native empty skeletal complete material slots");
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
Obj material(Obj component,uint32_t slot,HsmpViewResult* r,bool nullable=false) {
    Function f(L"/Script/Engine.PrimitiveComponent:GetMaterial"); f.put(L"ElementIndex",L"IntProperty",static_cast<int32_t>(slot));
    f.call(component,r); auto out=f.returned(); require(nullable||out.weak,"native material slot unavailable"); return out;
}
void source_static(Obj component,const HsmpViewComponent& c,HsmpViewResult* r) {
    const bool visible=effective_visible(component);
    require(visible==(c.visible!=0),"source visibility recipe changed");
    for(uint32_t i=0;i<c.material_count;++i) {
        if(c.kind==9&&text(c.materials[i].base).empty()){require(!material(component,c.materials[i].slot,r,true).weak,"native original null material replaced");continue;}
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
        if(c.kind==9&&text(c.materials[i].base).empty()){require(!material(component,c.materials[i].slot,r,true).weak,"native source null material changed");continue;}
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
struct Part {uint32_t id{},kind{};Obj render{},leader{};std::vector<Obj> materials;Obj native_asset{};std::wstring arm_socket;HsmpViewSpringArmFrame arm{};std::optional<PoseBinding> pose;std::optional<MeshBinding> mesh;std::optional<ArmPublication> arm_publication;};
struct Mirror {Obj world{},actor{};std::vector<Part> parts;bool diagnostic_applied{};};
struct MeshWatch;
thread_local const MeshWatch* active_mesh_watch{};
struct MeshWatch {
    const MeshWatch* previous{active_mesh_watch};std::vector<const Mirror*> mirrors;const Part* current{};
    bool immutable{};std::shared_ptr<const MeshPlan> plan;
    MeshWatch(std::vector<const Mirror*> value,const Part* part=nullptr,bool stable=false):mirrors(std::move(value)),current(part),immutable(stable){
        if(immutable)plan=mesh_watch_plan(this);active_mesh_watch=this;
    }
    ~MeshWatch(){active_mesh_watch=previous;}
};
void mesh_call_guard(){
    CaptureTimer present_mesh_time(1,present_provider_active);
    if(!active_mesh_watch)return;
    if(active_mesh_watch->plan){mesh_plan_final(*active_mesh_watch->plan);return;}
    std::vector<const MeshBinding*> bindings;
    for(auto* watch=active_mesh_watch;watch;watch=watch->previous){
        for(const auto* mirror:watch->mirrors)for(const auto& part:mirror->parts)if(part.mesh)bindings.push_back(&*part.mesh);
        if(watch->current&&watch->current->mesh)bindings.push_back(&*watch->current->mesh);
    }
    if(!bindings.empty())mesh_bindings_final(bindings);
}
void forget_owned_materials(Obj actor){
    for(auto it=vertex_source_materials.begin();it!=vertex_source_materials.end();){
        if(it->second.owned==1&&same(it->second.owner,actor))it=vertex_source_materials.erase(it);else ++it;}
}
void forget_mirror_materials(const Mirror& mirror){
    for(const auto& part:mirror.parts){const auto found=vertex_source_materials.find(part.render.weak);
        if(found!=vertex_source_materials.end()&&found->second.owned==1&&same(found->second.owner,mirror.actor)&&same(found->second.component,part.render))vertex_source_materials.erase(found);}
}
std::map<uint64_t,Mirror> mirrors;
std::mutex mirror_mutex;
uint64_t next_mirror{1};
void destroy_actor(Obj world,Obj actor) {
    require(same(actor_world(actor),world),"mirror destroy world mismatch");Function f(L"/Script/Engine.Actor:K2_DestroyActor");
    // Destroy invalidates the object, so only qualify immediately before the call.
    require(vt->is_a(get(actor),get(f.cls))!=0,"mirror destroy class");
    void* object=get(actor);void* function=get(f.function);check_guard();lookup_finish();
    vt->call(object,function,f.buf.data());
    // The actor may now be dead. Only the borrowed world scope is checked.
    check_guard();
}
void initialize_result(HsmpViewResult* r) {require(r!=nullptr,"native result missing");*r={};}
void failure(HsmpViewResult* r,const char* why) {if(r){r->complete=0;std::snprintf(r->reason,sizeof(r->reason),"%s",why);}}
int32_t inspect(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewResult* r) {
    try{initialize_result(r);thread();OperationScope scope(guard,world);supported(world,owner,component,r);lookup_finish();r->complete=1;return 1;}
    catch(const std::exception& e){failure(r,e.what());return -1;}
}
void capture_body(Obj world,Obj owner,Obj component,const HsmpViewComponent* c,HsmpViewFrame* out,CaptureTrace& trace,HsmpViewResult* r) {
    require(c&&out,"native capture arguments");trace.label(*c);layouts();frame(*c,*out);SplineOperation spline_scope(c->kind==5?owner:Obj{},c->kind==5?component:Obj{});VertexOperation vertex_scope(c->vertex_state==0||c->kind==8||c->kind==9?owner:Obj{},c->vertex_state==0||c->kind==8||c->kind==9?component:Obj{},c->kind==8||c->kind==9);SceneOperation scene_scope(c->kind==6||c->kind==7?owner:Obj{},c->kind==6||c->kind==7?component:Obj{},c->kind);supported(world,owner,component,r,c->kind==9);
        if(c->kind==9)vertex_bind_source_materials(world,owner,component);
        if(c->kind==0) {require(same(mesh_asset(component,r),asset(c->asset,L"/Script/Engine.SkeletalMesh")),"source mesh recipe changed");}
        else if(c->kind>=4)require(same(keep(vt->class_of(get(component))),asset(c->asset,L"/Script/CoreUObject.Class")),"source scene anchor class changed");
        else require(same(object_property(component,L"StaticMesh"),asset(c->asset,L"/Script/Engine.StaticMesh")),"source static mesh recipe changed");
        trace.mark(1);
        if(c->kind!=5)source_static(component,*c,r);if(c->kind<=1)verify_colors(component,*c,r);
        vertex_native_asset(world,owner,component,*c,r);
        trace.mark(2);
        capture_values(component,*c,*out,r);qualify(world,owner,component,r);
        if(c->kind==5)spline_capture(world,owner,component,c->spline,*out->spline,r);
        if(c->kind==4){scene_anchor(component);source_static(component,*c,r);}
        if(c->kind==7)*out->spring_arm=arm_observe(world,owner,component,c->spring_arm_socket,r);
        trace.mark(3);
        vertex_native_asset(world,owner,component,*c,r);
    lookup_finish();trace.mark(4);trace.row.complete=1;
}
int32_t capture(Obj world,Obj owner,Obj component,const HsmpViewComponent* c,HsmpViewFrame* out,const HsmpViewGuard* guard,HsmpViewResult* r) {
    CaptureTrace trace(active_capture_watch==nullptr);
    try{initialize_result(r);thread();OperationScope scope(guard,world);capture_body(world,owner,component,c,out,trace,r);r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
uint64_t create(Obj world,const HsmpViewComponent* recipes,uint32_t count,const HsmpViewGuard* guard,HsmpViewResult* r) {
    CreateTrace trace;
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
        MeshWatch mesh_watch({&mirror});
        for(uint32_t i=0;i<count;++i) {
            const auto& c=recipes[i];Part part{c.id,c.kind,{},{},{}};
            MeshWatch part_mesh_watch({},&part);
            trace.part(c.id,c.kind);
            require(std::none_of(mirror.parts.begin(),mirror.parts.end(),[&](const Part& p){return p.id==c.id;}),"duplicate mirror component id");
            if(c.kind==0) {
                part.mesh=mesh_binding(world,actor,c.asset,r);auto mesh=part.mesh->pinned;
                part.leader=add_component(actor,L"/Script/Engine.PoseableMeshComponent",c.relative,r);
                pose_profile(part.leader,true);
                Function leader_mesh(L"/Script/Engine.SkinnedMeshComponent:SetSkinnedAssetAndUpdate");leader_mesh.object(L"NewMesh",mesh);leader_mesh.boolean(L"bReinitPose",true);leader_mesh.call(part.leader,r);
                mesh=mesh_admit(*part.mesh,part.leader,c.asset,true,"calc_set",r);
                finish_component(actor,part.leader,c.relative,r);visibility(part.leader,false,r);
                part.render=add_component(actor,L"/Script/Engine.SkeletalMeshComponent",c.relative,r);
                pose_profile(part.render,false);
                Function disable_pp(L"/Script/Engine.SkeletalMeshComponent:SetDisablePostProcessBlueprint");disable_pp.boolean(L"bInDisablePostProcess",true);disable_pp.call(part.render,r);
                Function anim(L"/Script/Engine.SkeletalMeshComponent:SetAnimClass");anim.object(L"NewClass",{},true);anim.call(part.render,r);
                Function set_mesh(L"/Script/Engine.SkeletalMeshComponent:SetSkeletalMeshAsset");set_mesh.object(L"NewMesh",mesh);set_mesh.call(part.render,r);
                mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"mesh_set",r);
                Function allow_cloth(L"/Script/Engine.SkeletalMeshComponent:SetAllowClothActors");allow_cloth.boolean(L"bInAllow",false);allow_cloth.call(part.render,r);
                mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"cloth_allow",r);
                Function suspend(L"/Script/Engine.SkeletalMeshComponent:SuspendClothingSimulation");suspend.call(part.render,r);
                mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"cloth_suspend",r);
                finish_component(actor,part.render,c.relative,r);mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"finish",r);
                require(!object_property(part.render,L"AnimScriptInstance").weak&&!object_property(part.render,L"PostProcessAnimInstance").weak,"mirror animation instance active");
                scene_inert(part.leader,r);mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"calc_inert",r);
                scene_inert(part.render,r);mesh=mesh_admit(*part.mesh,part.render,c.asset,false,"render_inert",r);
                mesh=mesh_admit(*part.mesh,part.leader,c.asset,true,"verified",r);
                for(uint32_t b=0;b<c.hidden_count;++b)for(auto target:{part.leader,part.render}) {
                    Function hide(L"/Script/Engine.SkinnedMeshComponent:HideBoneByName");hide.put(L"BoneName",L"NameProperty",name(c.hidden_bones[b]));hide.enumeration(L"PhysBodyOption",0);hide.call(target,r);
                    Function check(L"/Script/Engine.SkinnedMeshComponent:IsBoneHiddenByName");check.put(L"BoneName",L"NameProperty",name(c.hidden_bones[b]));check.call(target,r);
                    auto p=check.field(L"ReturnValue",L"BoolProperty",1);require((check.buf[static_cast<size_t>(p.offset+p.bool_offset)]&p.bool_mask)!=0,"mirror hidden bone readback failed");
                }
                trace.emit("pose_bind",0);part.pose=pose_bind(world,actor,part.leader,part.render,mesh,c.bone_count,r);trace.emit("pose_bind",1);
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
            }else if(c.kind==8||c.kind==9){
                part.render=add_component(actor,c.kind==9?L"/Script/Engine.SkeletalMeshComponent":L"/Script/Engine.StaticMeshComponent",c.relative,r);
                VertexOperation operation(actor,part.render,true);require(vertex_empty,"native empty mirror constructor asset present");
                if(c.kind==9){
                    Function disable_pp(L"/Script/Engine.SkeletalMeshComponent:SetDisablePostProcessBlueprint");disable_pp.boolean(L"bInDisablePostProcess",true);disable_pp.call(part.render,r);
                    Function anim(L"/Script/Engine.SkeletalMeshComponent:SetAnimClass");anim.object(L"NewClass",{},true);anim.call(part.render,r);
                    Function cloth(L"/Script/Engine.SkeletalMeshComponent:SetAllowClothActors");cloth.boolean(L"bInAllow",false);cloth.call(part.render,r);
                    Function suspend(L"/Script/Engine.SkeletalMeshComponent:SuspendClothingSimulation");suspend.call(part.render,r);
                    Function suspended(L"/Script/Engine.SkeletalMeshComponent:IsClothingSimulationSuspended");suspended.call(part.render,r);const auto flag=suspended.field(L"ReturnValue",L"BoolProperty",1);
                    require((suspended.buf[static_cast<size_t>(flag.offset+flag.bool_offset)]&flag.bool_mask)!=0,"native empty mirror cloth not suspended");
                    Function mesh(L"/Script/Engine.SkeletalMeshComponent:SetSkeletalMeshAsset");mesh.object(L"NewMesh",{});mesh.call(part.render,r);
                    Function follow(L"/Script/Engine.SkinnedMeshComponent:SetLeaderPoseComponent");follow.object(L"NewLeaderBoneComponent",{});follow.boolean(L"bForceUpdate",true);follow.boolean(L"bInFollowerShouldTickPose",false);follow.call(part.render,r);
                    require(!object_property(part.render,L"AnimScriptInstance").weak&&!object_property(part.render,L"PostProcessAnimInstance").weak,"native empty mirror animation instance active");
                }else{Function mesh(L"/Script/Engine.StaticMeshComponent:SetStaticMesh");mesh.object(L"NewMesh",{});mesh.call(part.render,r);}
                finish_component(actor,part.render,c.relative,r);scene_inert(part.render,r);
            }else {
                part.render=add_component(actor,L"/Script/Engine.StaticMeshComponent",c.relative,r);
                auto mesh=asset(c.asset,L"/Script/Engine.StaticMesh");Function set_mesh(L"/Script/Engine.StaticMeshComponent:SetStaticMesh");set_mesh.object(L"NewMesh",mesh);set_mesh.call(part.render,r);
                if(c.vertex_state==0){part.native_asset=mesh;VertexOperation vertex_scope(actor,part.render);finish_component(actor,part.render,c.relative,r);}
                else finish_component(actor,part.render,c.relative,r);
                require(same(object_property(part.render,L"StaticMesh"),mesh),"mirror static asset readback failed");
            }
            std::optional<VertexOperation> vertex_scope;
            if(c.vertex_state==0||c.kind==8||c.kind==9)vertex_scope.emplace(actor,part.render,c.kind==8||c.kind==9);
            std::optional<SceneOperation> scene_scope;if(c.kind==6||c.kind==7)scene_scope.emplace(actor,part.render,c.kind);
            for(uint32_t j=0;j<c.material_count;++j) {
                if(c.kind==9&&text(c.materials[j].base).empty()){
                    Function clear(L"/Script/Engine.PrimitiveComponent:SetMaterial");clear.put(L"ElementIndex",L"IntProperty",static_cast<int32_t>(c.materials[j].slot));clear.object(L"Material",{});clear.call(part.render,r);
                    require(!material(part.render,c.materials[j].slot,r,true).weak,"native mirror null material assignment failed");part.materials.push_back({});continue;}
                const auto& m=c.materials[j];Function mid(L"/Script/Engine.PrimitiveComponent:CreateDynamicMaterialInstance");mid.put(L"ElementIndex",L"IntProperty",static_cast<int32_t>(m.slot));mid.object(L"SourceMaterial",asset(m.base,L"/Script/Engine.MaterialInterface"));mid.put(L"OptionalName",L"NameProperty",uint64_t{});mid.call(part.render,r);
                auto instance=mid.returned();require(instance.weak&&is(instance,L"/Script/Engine.MaterialInstanceDynamic")&&same(material(part.render,m.slot,r),instance),"mirror material creation failed");part.materials.push_back(instance);
            }
            if(c.kind<=1)colors(part.render,c,r);visibility(part.render,c.visible!=0,r);qualify(world,actor,part.render,r);vertex_native_asset(world,actor,part.render,c,r);
            if(c.kind==9)vertex_bind_source_materials(world,actor,part.render,1,&part.materials);
            if(part.leader.weak)qualify(world,actor,part.leader,r);mirror.parts.push_back(std::move(part));
        }
        // Source roots have no parent; every other direct parent must be in the
        // complete dictionary. Scene anchors preserve their native sockets.
        for(const auto i:order) {
            const auto& c=recipes[i];if(c.parent==0)continue;
            trace.component=c.id;trace.kind=c.kind;trace.emit("attach",0);
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
        trace.emit("finish_set",0);finish_scene_set(world,targets,r);trace.emit("finish_set",1);
        for(const auto& part:mirror.parts)if(part.pose){pose_pure(*part.pose);pose_buffers(vertex_pure(part.render),part.pose->count);}
        for(const auto& part:mirror.parts)if(part.mesh)mesh_binding_final(*part.mesh);
        lookup_finish();require(next_mirror!=0,"mirror handle exhausted");const auto id=next_mirror++;mirrors.emplace(id,std::move(mirror));r->complete=1;trace.terminal(1);return id;
    }catch(const std::exception& e){trace.terminal(2);failure(r,e.what());if(actor.weak){forget_owned_materials(actor);try{OperationScope scope(guard,world);destroy_actor(world,actor);}catch(const std::exception&){}}return 0;}
}
void world_transform(Obj component,const Transform& t,HsmpViewResult* r) {
    Function f(L"/Script/Engine.SceneComponent:K2_SetWorldTransform");f.put(L"NewTransform",L"StructProperty",engine(t),L"Transform");f.boolean(L"bSweep",false);f.boolean(L"bTeleport",true);f.call(component,r);
}
bool close(const Transform& a,const Transform& b) {
    for(size_t i=0;i<3;++i) if(std::abs(a.p[i]-b.p[i])>0.001||std::abs(a.scale[i]-b.scale[i])>0.00001)return false;
    double direct{},opposite{};for(size_t i=0;i<4;++i){direct+=std::abs(a.q[i]-b.q[i]);opposite+=std::abs(a.q[i]+b.q[i]);}
    return std::min(direct,opposite)<=0.00001;
}
void world_readback(Obj component,const HsmpViewComponent& recipe,const Transform& expected,const char* stage,HsmpViewResult* r){
    const auto actual=transform(component,L"/Script/Engine.SceneComponent:K2_GetComponentToWorld",r);
    if(close(actual,expected))return;
    double position{},rotation{},opposite_rotation{},scale{},direct{},opposite{};
    for(size_t i=0;i<3;++i){position=std::max(position,std::abs(actual.p[i]-expected.p[i]));scale=std::max(scale,std::abs(actual.scale[i]-expected.scale[i]));}
    for(size_t i=0;i<4;++i){const auto difference=std::abs(actual.q[i]-expected.q[i]),opposite_difference=std::abs(actual.q[i]+expected.q[i]);
        rotation=std::max(rotation,difference);opposite_rotation=std::max(opposite_rotation,opposite_difference);direct+=difference;opposite+=opposite_difference;}
    const auto socket=text(recipe.socket);char label[25]{};const auto count=std::min(socket.size(),sizeof(label)-1);
    for(size_t i=0;i<count;++i){const auto ch=socket[i];label[i]=ch>=32&&ch<127&&ch!='"'&&ch!='\\'?static_cast<char>(ch):'?';}
    char reason[192]{};std::snprintf(reason,sizeof(reason),"mirror world transform readback failed; stage=%s id=%u kind=%u parent=%u dp=%.17g dq=%.17g ds=%.17g qe=%.17g socket=%s",
        stage,recipe.id,recipe.kind,recipe.parent,position,std::min(rotation,opposite_rotation),scale,std::min(direct,opposite),label);throw Error(reason);
}
template<class T> void material_set(Obj mat,const HsmpViewParameter& p,const T& value,const wchar_t* path,const wchar_t* type,const wchar_t* sub,HsmpViewResult* r) {
    Function f(path);f.put(L"ParameterInfo",L"StructProperty",parameter(p),L"MaterialParameterInfo");f.put(L"Value",type,value,sub);f.call(mat,r);
}
int32_t apply(Obj world,uint64_t id,const HsmpViewComponent* recipes,const HsmpViewFrame* frames,uint32_t count,const HsmpViewGuard* guard,HsmpViewResult* r) {
    PresentProviderTrace diagnostic;
    const std::lock_guard lock(mirror_mutex);
    try{initialize_result(r);thread();OperationScope scope(guard,world);get(world);auto it=mirrors.find(id);require(it!=mirrors.end(),"mirror handle missing");auto& mirror=it->second;
        diagnostic.begin(mirror.diagnostic_applied);
        MeshWatch mesh_watch({&mirror},nullptr,true);
        require(same(mirror.world,world)&&same(actor_world(mirror.actor,r),world)&&count==mirror.parts.size()&&pointers(recipes,count,64)&&pointers(frames,count,64),"mirror apply scope");
        const auto order=parent_order(recipes,count);
        std::vector<ArmOwnedChild> arm_owned;arm_owned.reserve(mirror.parts.size()*2);
        if(std::any_of(mirror.parts.begin(),mirror.parts.end(),[](const Part& part){return part.kind==7;}))for(size_t i=0;i<mirror.parts.size();++i){
            Obj parent{};if(recipes[i].parent){const auto found=std::find_if(mirror.parts.begin(),mirror.parts.end(),[&](const Part& part){return part.id==recipes[i].parent;});require(found!=mirror.parts.end(),"native spring owned parent missing");parent=found->render;}
            const auto socket=name(recipes[i].socket);arm_owned.push_back({mirror.parts[i].render,parent,socket});if(mirror.parts[i].leader.weak)arm_owned.push_back({mirror.parts[i].leader,parent,socket});}
        std::vector<std::pair<size_t,PosePublished>> published_poses;
        for(const auto i:order) {
            const auto& c=recipes[i];const auto& f=frames[i];auto& part=mirror.parts[i];frame(c,f);require(c.id==part.id&&c.kind==part.kind,"mirror recipe generation mismatch");
            SplineOperation spline_scope(c.kind==5?mirror.actor:Obj{},c.kind==5?part.render:Obj{});
            VertexOperation vertex_scope(c.vertex_state==0||c.kind==8||c.kind==9?mirror.actor:Obj{},c.vertex_state==0||c.kind==8||c.kind==9?part.render:Obj{},c.kind==8||c.kind==9);
            SceneOperation scene_scope(c.kind==6||c.kind==7?mirror.actor:Obj{},c.kind==6||c.kind==7?part.render:Obj{},c.kind);
            vertex_native_asset(world,mirror.actor,part.render,c,r);
            qualify(world,mirror.actor,part.render,r);world_transform(part.render,f.world,r);world_readback(part.render,c,f.world,"world_set",r);
            if(part.leader.weak)world_transform(part.leader,f.world,r);
            if(c.kind==5)spline_apply(world,mirror.actor,part.render,c.spline,*f.spline,r);
            if(c.kind==7){
                part.arm_publication=arm_apply(world,mirror.actor,part.render,c.spring_arm_socket,*f.spring_arm,arm_owned,r);part.arm=*f.spring_arm;
            }
            for(uint32_t j=0;j<c.bone_count;++j) {
                Function b(L"/Script/Engine.PoseableMeshComponent:SetBoneTransformByName");b.put(L"BoneName",L"NameProperty",name(c.bones[j]));b.put(L"InTransform",L"StructProperty",engine(f.bones[j]),L"Transform");b.enumeration(L"BoneSpace",1);b.call(part.leader,r);
            }
            if(c.kind==0){require(part.pose.has_value(),"native mirror pose binding missing");published_poses.emplace_back(i,pose_transfer(*part.pose,r));}
            for(uint32_t j=0;j<c.morph_count;++j) {
                require(std::isfinite(f.morphs[j]),"mirror morph invalid");Function m(L"/Script/Engine.SkeletalMeshComponent:SetMorphTarget");m.put(L"MorphTargetName",L"NameProperty",name(c.morphs[j]));m.put(L"Value",L"FloatProperty",f.morphs[j]);m.boolean(L"bRemoveZeroWeight",false);m.call(part.render,r);
            }
            uint32_t si{},vi{},ti{};
            for(uint32_t j=0;j<c.material_count;++j) {
                const auto& m=c.materials[j];auto mat=part.materials[j];require(same(material(part.render,m.slot,r,c.kind==9),mat),"mirror material replaced");
                for(uint32_t k=0;k<m.scalar_count;++k) {float value=f.scalars[si++];require(std::isfinite(value),"mirror material scalar invalid");material_set(mat,m.scalars[k],value,L"/Script/Engine.MaterialInstanceDynamic:SetScalarParameterValueByInfo",L"FloatProperty",nullptr,r);
                    require(material_get<float>(mat,m.scalars[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetScalarParameterValueByInfo",L"FloatProperty",nullptr,r)==value,"mirror scalar readback failed");}
                for(uint32_t k=0;k<m.vector_count;++k) {std::array<float,4> value{};std::copy_n(f.vectors+4*vi++,4,value.data());for(float x:value)require(std::isfinite(x),"mirror material vector invalid");material_set(mat,m.vectors[k],value,L"/Script/Engine.MaterialInstanceDynamic:SetVectorParameterValueByInfo",L"StructProperty",L"LinearColor",r);
                    require(material_get<std::array<float,4>>(mat,m.vectors[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetVectorParameterValueByInfo",L"StructProperty",L"LinearColor",r)==value,"mirror vector readback failed");}
                for(uint32_t k=0;k<m.texture_count;++k) {auto texture=f.textures[ti++];void* ptr=texture.weak?get(texture):nullptr;material_set(mat,m.textures[k],ptr,L"/Script/Engine.MaterialInstanceDynamic:SetTextureParameterValueByInfo",L"ObjectProperty",nullptr,r);
                    require(material_get<void*>(mat,m.textures[k],L"/Script/Engine.MaterialInstanceDynamic:K2_GetTextureParameterValueByInfo",L"ObjectProperty",nullptr,r)==ptr,"mirror texture readback failed");}
            }
            world_readback(part.render,c,f.world,"complete",r);
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
        finish_scene_set(world,targets,r);
        for(const auto& [index,published]:published_poses)pose_final(*mirror.parts[index].pose,published);
        for(const auto& part:mirror.parts)if(part.mesh)mesh_binding_final(*part.mesh);
        for(const auto& part:mirror.parts)if(part.arm_publication)arm_publication_final(*part.arm_publication);
        lookup_finish();r->complete=1;mirror.diagnostic_applied=true;diagnostic.complete=true;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
void destroy(Obj world,uint64_t id,const HsmpViewGuard* guard) {const std::lock_guard lock(mirror_mutex);try{thread();auto it=mirrors.find(id);if(it==mirrors.end())return;auto mirror=it->second;forget_mirror_materials(mirror);mirrors.erase(it);require(same(world,mirror.world),"mirror destroy scope");OperationScope scope(guard,world);destroy_actor(world,mirror.actor);lookup_finish();}catch(const std::exception&) {}}
void discard(uint64_t id) {const std::lock_guard lock(mirror_mutex);const auto found=mirrors.find(id);if(found!=mirrors.end()){forget_mirror_materials(found->second);mirrors.erase(found);}}
using ObjectFlags=const uint32_t*(*)(const void*);
ObjectFlags retirement_flags{};
Free retirement_free{};
using RetirementIndex=void*(*)(int32_t);
using RetirementSlotObject=void**(*)(void*);
using RetirementSlotSerial=int32_t*(*)(void*);
RetirementIndex retirement_index{};RetirementSlotObject retirement_object{};RetirementSlotSerial retirement_serial{};
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
struct LookupNodeEvidence {uint64_t address{},weak{};};
HsmpNativePathNode source_path_node(void* object,LookupNodeEvidence* evidence=nullptr,std::optional<uint64_t> observed_weak=std::nullopt) {
    require(vt&&object_name&&retirement_flags&&source_outer,"native path metadata unavailable");
    require(object!=nullptr,"native path node missing");HsmpNativePathNode node{};node.weak=observed_weak?*observed_weak:vt->weak(object);node.address=reinterpret_cast<uint64_t>(object);
    if(evidence)*evidence={node.address,node.weak};
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
void lookup_world_final(const LookupState& state){
    const auto* world=source_path_get(state.world_node);source_path_get(state.gi_node);void* gi{};
    std::memcpy(&gi,static_cast<const uint8_t*>(world)+state.gi_property.offset,8);
    require(reinterpret_cast<uint64_t>(gi)==state.gi.address,"native lookup original world/game-instance changed");
}
void lookup_start(LookupState& state,Obj world,Obj gi){
    state.world=world;state.gi=gi;state.gi_property=property(world,L"OwningGameInstance",L"ObjectProperty",8);
    state.world_node=source_path_node(get(world));
    state.gi_node=source_path_node(get(gi));check_guard();lookup_world_final(state);
}
void* lookup_zero_object(void* original_item,uint64_t address){
    require(original_item&&address&&retirement_index&&retirement_object&&retirement_serial,"native lookup zero-package slot metadata unavailable");
    void* item=retirement_index(0);require(item==original_item,"native lookup original zero-package slot changed");
    const auto* object=retirement_object(item);const auto* serial=retirement_serial(item);
    require(object&&serial&&reinterpret_cast<uint64_t>(*object)==address&&*serial==0,"native lookup original zero-package object/serial changed");return *object;
}
void* lookup_node_get(const HsmpNativePathNode& node,void* zero_item){
    if(node.weak)return source_path_get(node);
    require(vt&&object_name&&retirement_flags&&source_outer&&source_package_name,"native lookup zero-package metadata unavailable");
    void* object=lookup_zero_object(zero_item,node.address);
    const auto* flags=retirement_flags(object);require(flags&&(*flags&0x40000000u)==0,"native lookup zero-package object garbage");
    void* cls=vt->resolve(node.class_weak);require(node.class_weak&&cls&&reinterpret_cast<uint64_t>(cls)==node.class_address&&vt->weak(cls)==node.class_weak,"native lookup zero-package class slot/serial changed");
    flags=retirement_flags(cls);require(flags&&(*flags&0x40000000u)==0,"native lookup zero-package class garbage");
    const auto* object_name_value=object_name(object);const auto* class_name_value=object_name(cls);const auto* outer=source_outer(object);
    require(object_name_value&&*object_name_value==node.name&&class_name_value&&*class_name_value==node.class_name&&node.class_name==*source_package_name&&
        vt->class_of(object)==cls&&outer&&*outer==nullptr,"native lookup original zero-package FName/class/Outer changed");
    require(lookup_zero_object(zero_item,node.address)==object,"native lookup zero-package slot changed during qualification");return object;
}
HsmpNativePathNode lookup_node(void* object,LookupEntry& entry,bool ancestor,LookupNodeEvidence* evidence){
    require(vt&&object_name&&retirement_flags&&source_outer&&source_package_name,"native lookup path metadata unavailable");
    require(object!=nullptr,"native lookup path node missing");const auto weak=vt->weak(object);
    if(evidence)*evidence={reinterpret_cast<uint64_t>(object),weak};if(weak)return source_path_node(object,evidence,weak);
    require(ancestor&&retirement_index&&retirement_object&&retirement_serial,"native lookup zero-package ancestor slot unavailable");
    void* item=retirement_index(0);require(item!=nullptr,"native lookup zero-package slot unavailable");
    require(!entry.zero_item||entry.zero_item==item,"native lookup zero-package original slot changed");
    lookup_zero_object(item,reinterpret_cast<uint64_t>(object)); // before newly admitted class/FName/RF/Outer reads
    HsmpNativePathNode node{};node.address=reinterpret_cast<uint64_t>(object);void* cls=vt->class_of(object);
    require(cls!=nullptr,"native lookup zero-package class missing");node.class_weak=vt->weak(cls);node.class_address=reinterpret_cast<uint64_t>(cls);
    require(node.class_weak&&vt->resolve(node.class_weak)==cls,"native lookup zero-package class weak unavailable");
    const auto* own=object_name(object);const auto* class_name_value=object_name(cls);require(own&&class_name_value,"native lookup zero-package FName unavailable");
    node.name=*own;node.class_name=*class_name_value;lookup_node_get(node,item);entry.zero_item=item;return node;
}
void lookup_path_verify(const std::vector<HsmpNativePathNode>& nodes,const LookupEntry& entry){
    require(source_package_name&&*source_package_name==entry.package&&!nodes.empty()&&nodes.size()<=64,"native lookup original path/package changed");
    for(size_t i=0;i<nodes.size();++i){require(nodes[i].weak||i>0,"native lookup returned root weak unavailable");
        for(size_t j=0;j<i;++j)require(nodes[i].address!=nodes[j].address,"native lookup original path cycle");
        void* object=lookup_node_get(nodes[i],entry.zero_item);const auto* field=source_outer(object);require(field,"native lookup original Outer metadata unavailable");
        const void* outer{};std::memcpy(&outer,field,sizeof(outer));require(reinterpret_cast<uint64_t>(outer)==(i+1<nodes.size()?nodes[i+1].address:0),"native lookup original Outer changed");lookup_node_get(nodes[i],entry.zero_item);}
    require(*source_package_name==entry.package,"native lookup original package discriminator changed during walk");
}
void lookup_entry_final(const LookupEntry& entry){
    require(!entry.original.empty()&&entry.original.size()==entry.pinned.size(),"native lookup witness missing");
    lookup_path_verify(entry.original,entry);lookup_path_verify(entry.pinned,entry);
    for(size_t i=0;i<entry.pinned.size();++i){const auto& node=entry.pinned[i];const auto* p=lookup_node_get(node,entry.zero_item);const auto* flags=retirement_flags(p);const auto* cls=vt->resolve(node.class_weak);const auto* class_flags=retirement_flags(cls);
        require(flags&&class_flags&&*flags==entry.flags[i]&&*class_flags==entry.class_flags[i],"native lookup original RF/class flags changed");
        lookup_node_get(node,entry.zero_item);require(*flags==entry.flags[i]&&*class_flags==entry.class_flags[i],"native lookup RF/class flags changed during final qualification");}
}
uint64_t lookup_merge_weak(uint64_t original,uint64_t observed,uint64_t address){
    if(original==observed)return original;
    require(original&&observed,"native shared witness zero identity disagreement");
    if(mesh_serial_assignment({original,address},{observed,address}))return observed;
    require(mesh_serial_assignment({observed,address},{original,address}),"native shared witness original positive serial disagreement");return original;
}
void lookup_record(const LookupEntry& entry){
    require(active_lookup&&!entry.original.empty()&&entry.original.size()==entry.pinned.size()&&entry.flags.size()==entry.pinned.size()&&entry.class_flags.size()==entry.pinned.size(),"native shared witness bounds");
    auto& witnesses=active_lookup->witnesses;
    require(!witnesses.package||*witnesses.package==entry.package,"native shared witness package disagreement");witnesses.package=entry.package;
    for(const auto* path:{&entry.original,&entry.pinned})for(size_t i=0;i<path->size();++i){const auto& node=(*path)[i];
        const uint64_t outer=i+1<path->size()?(*path)[i+1].address:0;
        const LookupObjectWitness value{node,outer,entry.flags[i],node.weak?nullptr:entry.zero_item};
        auto [object,inserted]=witnesses.objects.emplace(node.address,value);
        if(!inserted){auto& saved=object->second;
            require(saved.node.name==node.name&&saved.node.class_address==node.class_address&&saved.node.class_name==node.class_name&&saved.outer==outer&&saved.flags==value.flags&&saved.zero_item==value.zero_item,"native shared object/path witness disagreement");
            saved.node.weak=lookup_merge_weak(saved.node.weak,node.weak,node.address);
            if(!node.weak)require(saved.node.class_weak==node.class_weak,"native shared zero-package class serial disagreement");
            else saved.node.class_weak=lookup_merge_weak(saved.node.class_weak,node.class_weak,node.class_address);
        }
        const LookupClassWitness cls{{node.class_weak,node.class_address},node.class_name,entry.class_flags[i],node.weak==0};
        auto [type,type_inserted]=witnesses.classes.emplace(node.class_address,cls);
        if(!type_inserted){auto& saved=type->second;require(saved.name==cls.name&&saved.flags==cls.flags,"native shared class witness disagreement");
            if(saved.exact_serial||cls.exact_serial)require(saved.object.weak==cls.object.weak,"native shared exact class serial disagreement");
            else saved.object.weak=lookup_merge_weak(saved.object.weak,cls.object.weak,node.class_address);
            saved.exact_serial=saved.exact_serial||cls.exact_serial;
        }
        constexpr size_t maximum=(512+32*64*5+4)*64; // existing path, frame-row and hierarchy bounds
        require(witnesses.objects.size()<=maximum&&witnesses.classes.size()<=maximum,"native shared witness aggregate bound");
    }
}
void lookup_witness_finish(){
    require(active_lookup&&vt&&object_name&&retirement_flags&&source_outer&&source_package_name,"native shared witness metadata unavailable");
    const auto& witnesses=active_lookup->witnesses;
    require(!witnesses.package||*source_package_name==*witnesses.package,"native shared original package changed");
    // Fresh native reads on every invocation; these copied expectations never
    // become a ticket across check_guard, a getter, nested scope or dispatch.
    for(const auto& [address,cls]:witnesses.classes){void* p=vt->resolve(cls.object.weak);
        require(cls.object.weak&&reinterpret_cast<uint64_t>(p)==address,"native shared original class slot changed");
        const auto* flags=retirement_flags(p);const auto* n=object_name(p);
        require(flags&&*flags==cls.flags&&(*flags&0x40000000u)==0&&n&&*n==cls.name&&(!cls.exact_serial||vt->weak(p)==cls.object.weak),"native shared original class metadata changed");}
    for(const auto& [address,saved]:witnesses.objects){const auto& node=saved.node;
        void* p=node.weak?vt->resolve(node.weak):lookup_zero_object(saved.zero_item,address);
        require(reinterpret_cast<uint64_t>(p)==address,"native shared original object slot changed");
        const auto* flags=retirement_flags(p);const auto* n=object_name(p);const auto* outer=source_outer(p);
        require(flags&&*flags==saved.flags&&(*flags&0x40000000u)==0&&n&&*n==node.name&&vt->class_of(p)==reinterpret_cast<void*>(node.class_address)&&outer&&reinterpret_cast<uint64_t>(*outer)==saved.outer,"native shared original object/path metadata changed");
        if(!node.weak)require(node.class_name==*source_package_name&&saved.outer==0&&lookup_zero_object(saved.zero_item,address)==p,"native shared original zero-package changed");
    }
    require(!witnesses.package||*source_package_name==*witnesses.package,"native shared package changed during pure boundary");
}
void lookup_finish(){CaptureTimer present_lookup_time(2,present_provider_active);if(!active_lookup)return;lookup_world_final(*active_lookup);lookup_witness_finish();capture_watch_links();lookup_witness_finish();lookup_world_final(*active_lookup);}
void lookup_remember(const HsmpNativePathNode& node){
    const Identity value{node.address,node.name,node.class_weak,node.class_address};const auto found=identities.find(node.weak);
    if(found==identities.end()){require(identities.size()<65536,"native lookup identity bound");identities.emplace(node.weak,value);}
    else require(found->second.address==value.address&&found->second.name==value.name&&found->second.class_address==value.class_address,"native lookup retained identity changed");
}
void lookup_pin(LookupEntry& entry){
    lookup_entry_final(entry);auto candidate=entry.pinned;
    for(auto& node:candidate){auto* p=lookup_node_get(node,entry.zero_item);const Obj old{node.weak,node.address},current{vt->weak(p),node.address};
        if(!node.weak){require(current.weak==0,"native lookup zero-package serial assignment unsupported");continue;}
        require(same(old,current)||mesh_serial_assignment(old,current),"native lookup positive object serial changed");node.weak=current.weak;
        void* cls=vt->resolve(node.class_weak);const Obj old_class{node.class_weak,node.class_address},current_class{vt->weak(cls),node.class_address};
        require(same(old_class,current_class)||mesh_serial_assignment(old_class,current_class),"native lookup positive class serial changed");node.class_weak=current_class.weak;}
    LookupEntry proposed=entry;proposed.pinned=std::move(candidate);lookup_entry_final(proposed);lookup_world_final(*active_lookup);
    for(const auto& node:proposed.pinned){if(node.weak)lookup_remember(node);const auto class_node=source_path_node(vt->resolve(node.class_weak));require(class_node.name==node.class_name&&class_node.address==node.class_address,"native lookup retained class identity changed");lookup_remember(class_node);}
    lookup_entry_final(proposed);lookup_world_final(*active_lookup);entry.pinned=std::move(proposed.pinned);lookup_record(entry);
}
[[noreturn]] void lookup_cold_failure(const Error& error,const char* stage,uint32_t depth,Obj root,const LookupNodeEvidence& node){
    // Diagnostic-only slot access uses the admitted FUObjectItem getters. The
    // returned slot object is compared as a scalar and is never dereferenced.
    int available=-1,item_present=-1,slot_match=-1,serial_zero=-1;
    if(node.weak==0&&node.address){available=retirement_index&&retirement_object&&retirement_serial?1:0;
        if(available){void* item=retirement_index(0);item_present=item?1:0;if(item){const auto* object=retirement_object(item);const auto* serial=retirement_serial(item);
            if(object)slot_match=reinterpret_cast<uint64_t>(*object)==node.address?1:0;if(serial)serial_zero=*serial==0?1:0;}}}
    char reason[192]{};std::snprintf(reason,sizeof(reason),"%s; stage=%s n=%u w=%llx rw=%llx idx0=%d/%d/%d/%d a=%llx r=%llx",
        error.what(),stage,depth,static_cast<unsigned long long>(node.weak),static_cast<unsigned long long>(root.weak),available,item_present,slot_match,serial_zero,
        static_cast<unsigned long long>(node.address),static_cast<unsigned long long>(root.address));throw Error(reason);
}
Obj lookup_find(const wchar_t* path){
    check_guard();const std::wstring key(path);if(active_lookup){const auto hit=active_lookup->entries.find(key);
        if(hit!=active_lookup->entries.end()){lookup_trace_request(path,0);lookup_pin(hit->second);check_guard();lookup_entry_final(hit->second);lookup_world_final(*active_lookup);const auto& root=hit->second.pinned.front();return {root.weak,root.address};}}
    lookup_trace_request(path,active_lookup?1u:2u);
    profile_tick(2);void* object{};{LookupNativeTrace diagnostic(path);CaptureTimer timer(6);object=vt->find(u16(path));}const auto original=keep(object);
    if(!active_lookup)return original;
    // Cache capacity limits reuse only; an admitted larger operation continues
    // through the unchanged native lookup path without a new scene bound.
    if(active_lookup->entries.size()>=512){lookup_trace_request(path,3);return original;}
    const char* stage="package";uint32_t depth{};LookupNodeEvidence evidence{};
    try{
    require(source_package_name!=nullptr,"native lookup package metadata unavailable");
    stage="root_node";LookupEntry entry;entry.package=*source_package_name;auto node=lookup_node(get(original),entry,false,&evidence);const auto first_observed=node;node.weak=original.weak;
    for(;;){require(entry.original.size()<64,"native lookup full path bound");for(const auto& prior:entry.original)require(prior.address!=node.address,"native lookup path cycle");entry.original.push_back(node);
        const auto* p=lookup_node_get(node,entry.zero_item);const auto* flags=retirement_flags(p);const auto* class_flags=retirement_flags(vt->resolve(node.class_weak));require(flags&&class_flags,"native lookup RF metadata unavailable");entry.flags.push_back(*flags);entry.class_flags.push_back(*class_flags);
        const auto* outer=source_outer(p);require(outer,"native lookup Outer metadata unavailable");if(!*outer)break;stage="outer_node";++depth;evidence={reinterpret_cast<uint64_t>(*outer),0};node=lookup_node(const_cast<void*>(*outer),entry,true,&evidence);}
    stage="path_close";
    entry.pinned=entry.original;entry.pinned.front()=first_observed;lookup_pin(entry);check_guard();lookup_entry_final(entry);
    // The first lookup may be followed by a callback before its witness is
    // complete. A second exact native lookup closes the requested-key binding.
    stage="exact_find";profile_tick(2);void* second{};{LookupNativeTrace diagnostic(path);CaptureTimer timer(6);second=vt->find(u16(path));}const auto exact=keep(second);
    const Obj pinned{entry.pinned.front().weak,entry.pinned.front().address};
    require(same(pinned,exact)||mesh_serial_assignment(pinned,exact),"native lookup exact requested path changed");
    // Preserve the first positive serial observed by the second exact lookup
    // before another guard callback can assign a different positive serial.
    stage="exact_pin";lookup_pin(entry);require(same(Obj{entry.pinned.front().weak,entry.pinned.front().address},exact),"native lookup first observed serial changed");
    check_guard();lookup_pin(entry);lookup_finish();const auto root=entry.pinned.front();active_lookup->entries.emplace(key,std::move(entry));return {root.weak,root.address};
    }catch(const Error& error){lookup_cold_failure(error,stage,depth,original,evidence);}
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
// Never acquire a fresh asset by path here. This pure closure verifies the
// original witness and, once assigned naturally, the retained positive serial.
void mesh_binding_final(const MeshBinding& b){
    source_path_get(b.world_node);const auto* owner=source_path_get(b.owner_node);const auto* level=source_path_get(b.level_node);
    require(source_outer(owner)&&*source_outer(owner)==level,"native mesh original owner level changed");
    void* world{};std::memcpy(&world,static_cast<const uint8_t*>(level)+0xc0,8);
    require(reinterpret_cast<uint64_t>(world)==b.world.address,"native mesh original world changed");
    source_path_verify(b.path.data(),b.count,b.package);
    auto pinned=b.path[0];pinned.weak=b.pinned.weak;pinned.address=b.pinned.address;
    const auto* p=source_path_get(pinned);const auto* flags=retirement_flags(p);
    require(flags&&*flags==b.flags&&(*flags&0x40u)==0,"native mesh original flags/runtime profile changed");
}
// Expectations and successful reads live for this one callback-free guard only.
// Each original path is represented; shared nodes/classes are read once per pass.
struct MeshBoundaryNode {HsmpNativePathNode node{};std::optional<uint64_t> outer;};
struct MeshBoundary {
    std::map<uint64_t,MeshBoundaryNode> nodes;
    std::map<uint64_t,std::pair<Obj,uint64_t>> classes;
    std::optional<uint64_t> package;
    void add(const HsmpNativePathNode& node,std::optional<uint64_t> outer=std::nullopt){
        require(node.weak&&node.address&&node.class_weak&&node.class_address,"native mesh shared witness missing");
        auto [saved,inserted]=nodes.emplace(node.address,MeshBoundaryNode{node,outer});
        if(!inserted){auto& old=saved->second;
            require(old.node.name==node.name&&old.node.class_address==node.class_address&&old.node.class_name==node.class_name&&
                (!old.outer||!outer||*old.outer==*outer),"native mesh shared original witness disagreement");
            old.node.weak=lookup_merge_weak(old.node.weak,node.weak,node.address);
            old.node.class_weak=lookup_merge_weak(old.node.class_weak,node.class_weak,node.class_address);
            if(outer)old.outer=outer;
        }
        auto [cls,class_inserted]=classes.emplace(node.class_address,std::make_pair(Obj{node.class_weak,node.class_address},node.class_name));
        if(!class_inserted){require(cls->second.second==node.class_name,"native mesh shared original class disagreement");
            cls->second.first.weak=lookup_merge_weak(cls->second.first.weak,node.class_weak,node.class_address);}
    }
    void validate()const{
        require(vt&&object_name&&retirement_flags&&source_outer&&source_package_name,"native mesh shared metadata unavailable");
        require(!package||*source_package_name==*package,"native mesh original package discriminator changed");
        for(const auto& [address,saved]:classes){void* p=vt->resolve(saved.first.weak);
            require(reinterpret_cast<uint64_t>(p)==address,"native mesh shared original class slot changed");
            const auto* flags=retirement_flags(p);require(flags&&(*flags&0x40000000u)==0,"native mesh shared original class garbage");
            const auto* n=object_name(p);require(n&&*n==saved.second,"native mesh shared original class changed");}
        for(const auto& [address,saved]:nodes){const auto& node=saved.node;void* p=vt->resolve(node.weak);
            require(reinterpret_cast<uint64_t>(p)==address,"native mesh shared original object slot changed");
            const auto* flags=retirement_flags(p);require(flags&&(*flags&0x40000000u)==0,"native mesh shared original object garbage");
            const auto* n=object_name(p);require(n&&*n==node.name&&vt->class_of(p)==reinterpret_cast<void*>(node.class_address),"native mesh shared original FName/class changed");
            if(saved.outer){const auto* field=source_outer(p);require(field&&reinterpret_cast<uint64_t>(*field)==*saved.outer,"native mesh shared original Outer changed");}
        }
        require(!package||*source_package_name==*package,"native mesh original package discriminator changed during walk");
    }
};
MeshBoundary mesh_boundary_build(const std::vector<const MeshBinding*>& bindings){
    require(bindings.size()<=32*64,"native mesh shared binding bound");MeshBoundary boundary;
    for(const auto* b:bindings){require(b&&b->count>0&&b->count<=b->path.size(),"native mesh original path bounds");
        require(!boundary.package||*boundary.package==b->package,"native mesh shared package disagreement");boundary.package=b->package;
        boundary.add(b->world_node);boundary.add(b->owner_node,b->level_node.address);boundary.add(b->level_node);
        for(uint32_t i=0;i<b->count;++i){for(uint32_t j=0;j<i;++j)require(b->path[i].address!=b->path[j].address,"native mesh original path cycle");
            boundary.add(b->path[i],i+1<b->count?b->path[i+1].address:0);}
        auto pinned=b->path[0];require(b->pinned.address==pinned.address,"native mesh original pinned address changed");pinned.weak=b->pinned.weak;boundary.add(pinned);
    }
    return boundary;
}
void mesh_boundary_final(const MeshBoundary& boundary,const std::vector<const MeshBinding*>& bindings){
    boundary.validate();
    for(const auto* b:bindings){
        const auto* owner=vt->resolve(b->owner_node.weak);const auto* level=vt->resolve(b->level_node.weak);
        require(reinterpret_cast<uint64_t>(owner)==b->owner_node.address&&reinterpret_cast<uint64_t>(level)==b->level_node.address,"native mesh original owner/level slot changed");
        const auto* outer=source_outer(owner);require(outer&&*outer==level,"native mesh original owner level changed");
        void* world{};std::memcpy(&world,static_cast<const uint8_t*>(level)+0xc0,8);
        require(reinterpret_cast<uint64_t>(world)==b->world.address,"native mesh original world changed");
        const auto* p=vt->resolve(b->pinned.weak);require(reinterpret_cast<uint64_t>(p)==b->pinned.address,"native mesh original pinned slot changed");
        const auto* flags=retirement_flags(p);require(flags&&*flags==b->flags&&(*flags&0x40u)==0,"native mesh original flags/runtime profile changed");
    }
    boundary.validate();
}
void mesh_bindings_final(const std::vector<const MeshBinding*>& bindings){
    const auto boundary=mesh_boundary_build(bindings);mesh_boundary_final(boundary,bindings);
}
struct MeshPlan {
    std::vector<MeshBinding> bindings;
    std::vector<const MeshBinding*> pointers;
    MeshBoundary boundary;
    explicit MeshPlan(const std::vector<const MeshBinding*>& originals){
        bindings.reserve(originals.size());for(const auto* b:originals){require(b!=nullptr,"native mesh plan original missing");bindings.push_back(*b);}
        pointers.reserve(bindings.size());for(const auto& b:bindings)pointers.push_back(&b);
        boundary=mesh_boundary_build(pointers);
    }
    MeshPlan(const MeshPlan&)=delete;
    MeshPlan& operator=(const MeshPlan&)=delete;
};
std::shared_ptr<const MeshPlan> mesh_watch_plan(const MeshWatch* watch){
    std::vector<const MeshBinding*> originals;
    for(auto* w=watch;w;w=w->previous){if(!w->immutable||w->current)return {};
        for(const auto* mirror:w->mirrors)for(const auto& part:mirror->parts)if(part.mesh)originals.push_back(&*part.mesh);}
    if(originals.empty())return {};
    return std::make_shared<MeshPlan>(originals);
}
void mesh_plan_final(const MeshPlan& plan){mesh_boundary_final(plan.boundary,plan.pointers);}
bool mesh_serial_assignment(Obj original,Obj current){
    return original.weak&&current.weak&&(original.weak>>32)==0&&static_cast<int32_t>(current.weak>>32)>0&&
        original.address==current.address&&static_cast<uint32_t>(original.weak)==static_cast<uint32_t>(current.weak);
}
MeshBinding mesh_binding(Obj world,Obj owner,HsmpViewText path,HsmpViewResult* r){
    MeshBinding b;b.world=world;b.owner=owner;
    b.world_node=source_path_node(get(world));b.world_node.weak=world.weak;
    b.owner_node=source_path_node(get(owner));b.owner_node.weak=owner.weak;
    const auto level=returned(owner,L"/Script/Engine.Actor:GetLevel",r);
    require(level.weak&&property(level,L"OwningWorld",L"ObjectProperty",8).offset==0xc0,"native mesh owner level layout");
    b.level_node=source_path_node(get(level));b.level_node.weak=level.weak;
    b.original=asset(path,L"/Script/Engine.SkeletalMesh");b.pinned=b.original;
    auto root=source_path_node(get(b.original));root.weak=b.original.weak;
    const auto* flags=retirement_flags(source_path_get(root));require(flags!=nullptr,"native mesh original flags unavailable");b.flags=*flags;
    char reason[192]{};require(source_path_reader(&root,1,1,b.path.data(),64,&b.count,&b.package,reason,sizeof(reason))==1,reason);
    const auto exact=find(text(path).c_str());
    require(same(b.original,exact)||mesh_serial_assignment(b.original,exact),"native mesh exact recipe binding changed");
    vertex_live(b.original);vertex_live(exact);require(same(actor_world(owner,r),world),"native mesh owner world changed");
    check_guard();
    mesh_binding_final(b);auto current=b.path[0];current.weak=exact.weak;current.address=exact.address;source_path_get(current);b.pinned=exact;return b;
}
Obj mesh_admit(MeshBinding& b,Obj component,HsmpViewText path,bool calculator,const char* stage,HsmpViewResult* r){
    qualify(b.world,b.owner,component,r);auto receiver=source_path_node(vertex_live(component));receiver.weak=component.weak;
    const auto actual=mesh_asset(component,r);
    if(!same(actual,b.pinned)&&!mesh_serial_assignment(b.pinned,actual))mesh_assignment_error(actual,b.pinned,path,calculator,stage);
    vertex_live(b.original);vertex_live(b.pinned);vertex_live(actual);qualify(b.world,b.owner,component,r);check_guard();
    auto candidate=b.path[0];candidate.weak=actual.weak;candidate.address=actual.address;
    source_path_get(candidate);mesh_binding_final(b);
    const auto* p=static_cast<const uint8_t*>(source_path_get(receiver));uint64_t owner{},mesh{},skinned{};
    std::memcpy(&owner,p+0x90,8);std::memcpy(&mesh,p+0x558,8);std::memcpy(&skinned,p+0x560,8);
    require(owner==b.owner.address&&mesh==b.original.address&&skinned==b.original.address,"native mesh original receiver owner/aliases changed");
    source_path_get(candidate);b.pinned=actual;return b.pinned;
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
struct RetirementSlot {void* item{};uint64_t object{};int32_t serial{};};
RetirementSlot retirement_slot_copy(Obj actor){
    require(retirement_index&&retirement_object&&retirement_serial,"native object-array slot API unavailable");
    RetirementSlot out{};out.item=retirement_index(static_cast<int32_t>(actor.weak));if(!out.item)return out;
    const auto object=retirement_object(out.item);const auto serial=retirement_serial(out.item);
    require(object&&serial,"native object-array slot metadata unavailable");
    out.object=reinterpret_cast<uint64_t>(*object);out.serial=*serial;return out;
}
void retirement_scope_pure(Obj object){
    require(vt&&retirement_flags&&object_name,"native retirement pure metadata unavailable");
    const auto found=identities.find(object.weak);require(found!=identities.end(),"native retirement scope identity missing");const auto& id=found->second;
    void* p=vt->resolve(object.weak);require(p&&reinterpret_cast<uint64_t>(p)==object.address&&id.address==object.address,"native retirement original scope expired");
    void* cls=vt->resolve(id.class_weak);require(cls&&reinterpret_cast<uint64_t>(cls)==id.class_address,"native retirement original scope class expired");
    const auto class_identity=identities.find(id.class_weak);require(class_identity!=identities.end(),"native retirement original class identity missing");
    const auto flags=retirement_flags(p),class_flags=retirement_flags(cls);const auto n=object_name(p),cn=object_name(cls);
    require(flags&&class_flags&&!((*flags|*class_flags)&mirrored_garbage)&&n&&*n==id.name&&cn&&*cn==class_identity->second.name&&vt->class_of(p)==cls,
        "native retirement original scope name/class/flags changed");
}
bool retirement_probe_after(const RetiredDriver& record,HsmpViewRetirement* result){
    const auto world=record.world,actor=record.actor;
    const bool listed=world_contains(world,record.identity,actor.address);
    const auto original_serial=static_cast<int32_t>(actor.weak>>32);
    const Obj cls{record.identity.class_weak,record.identity.class_address};
    // A prior successful retirement may outlive its positive weak serial. The
    // native resolver rejects that expired handle before loading the slot object.
    // Keep live qualification and initial retirement strict; never read a replacement.
    get(world);get(cls);check_guard();const auto slot=retirement_slot_copy(actor);
    if(original_serial>0&&slot.item&&slot.serial!=original_serial){
        require(!listed,"native expired retirement address still in world census");
        require(vt->resolve(actor.weak)==nullptr,"native expired retirement handle still resolves");
        require((flags_of(get(world))&mirrored_garbage)==0&&(flags_of(get(cls))&mirrored_garbage)==0,
            "native expired retirement world/class garbage");
        // Every callback-capable qualification precedes this final scalar closure.
        get(world);get(cls);check_guard();retirement_scope_pure(world);retirement_scope_pure(cls);const auto final_slot=retirement_slot_copy(actor);
        require(final_slot.item==slot.item&&final_slot.object==slot.object&&final_slot.serial==slot.serial&&
            vt->resolve(actor.weak)==nullptr,"native expired retirement slot changed during proof");
        retirement_scope_pure(world);retirement_scope_pure(cls);const auto tail=retirement_slot_copy(actor);
        require(tail.item==slot.item&&tail.object==slot.object&&tail.serial==slot.serial,
            "native expired retirement slot changed after resolution");
        result->weak_present=0;result->after.listed=0;result->after.known=4;result->alive_after=0;return true;
    }
    void* p=original_slot(actor,record.identity);result->weak_present=p?1:0;
    if(p)result->after=lifecycle(p,record.identity,record.layout);
    result->after.listed=listed;result->after.known|=4;
    const bool retired=!listed&&(!p||(result->after.flags&mirrored_garbage)!=0);
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
        // Retain scope class identities while the original driver is still live.
        get(keep(vt->class_of(get(world))));
        get(keep(vt->class_of(get({identity.class_weak,identity.class_address}))));
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
        lookup_finish();result->dispatched=1;vt->call(object,function,f.buf.data());
        // Never invoke actor GetWorld/GetLevel/ProcessEvent after dispatch.
        check_guard();require(retirement_after(world,actor,identity,layout,result),"native actor retirement not proved");
        lookup_finish();retired_drivers.emplace(actor.weak,RetiredDriver{world,actor,identity,result->before,layout});return 1;
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
        require(retirement_probe_after(record->second,result),"native original actor retirement no longer proved");lookup_finish();return 1;
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
        lookup_finish();result->qualified=1;return 1;
    }catch(const std::exception& e){if(result)std::snprintf(result->reason,sizeof(result->reason),"%s",e.what());return -1;}
}
int32_t describe_spline(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewSplineProfile* out,HsmpViewResult* r) {
    const StaticProfileTraceScope tracing;
    try{profile_phase("cpp_admission",0);initialize_result(r);thread();OperationScope scope(guard,world);require(out!=nullptr,"native spline profile output missing");profile_phase("cpp_admission",1);
        profile_phase("core_layout",0);layouts();profile_phase("core_layout",1);
        auto snapshot=spline_coherent(world,owner,component,r);*out=spline_profile(snapshot);spline_profile_valid(*out);lookup_finish();r->complete=1;return 1;}
    catch(const std::exception& e){failure(r,e.what());return -1;}
}
int32_t describe_vertex_state(Obj world,Obj owner,Obj component,const HsmpViewGuard* guard,HsmpViewVertexState* out,HsmpViewResult* r){
    try{initialize_result(r);thread();OperationScope scope(guard,world);require(out!=nullptr,"native vertex state output missing");
        *out=vertex_observe(world,owner,component,r);lookup_finish();r->complete=1;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
int32_t finish_scene_sets(Obj world,const HsmpViewFinishTarget* source,uint32_t source_count,const uint64_t* handles,uint32_t mirror_count,const HsmpViewGuard* guard,HsmpViewResult* r){
    PresentProviderTrace diagnostic(true);
    const std::lock_guard lock(mirror_mutex);
    try{initialize_result(r);thread();OperationScope scope(guard,world);
        diagnostic.begin(mirror_count!=0&&source_count==0);
        require(pointers(source,source_count,32*64)&&pointers(handles,mirror_count,32)&&(!source_count||!mirror_count),"native vertex complete set arguments");
        std::vector<HsmpViewFinishTarget> targets;std::vector<const Mirror*> mesh_mirrors;if(source_count)targets.assign(source,source+source_count);
        for(uint32_t i=0;i<mirror_count;++i){
            require(std::find(handles,handles+i,handles[i])==handles+i,"native vertex duplicate mirror handle");
            const auto found=mirrors.find(handles[i]);require(found!=mirrors.end()&&same(found->second.world,world),"native vertex mirror generation");
            mesh_mirrors.push_back(&found->second);
            for(const auto& part:found->second.parts){if(part.native_asset.weak)targets.push_back({found->second.actor,part.render,part.native_asset});
                else if(part.kind>=6)targets.push_back({found->second.actor,part.render,{},part.kind,1,{u16(part.arm_socket.c_str()),static_cast<uint32_t>(part.arm_socket.size()),0},part.arm});}
        }
        MeshWatch mesh_watch(mesh_mirrors,nullptr,true);finish_scene_set(world,targets,r);
        for(const auto* mirror:mesh_mirrors)for(const auto& part:mirror->parts)if(part.mesh){mesh_binding_final(*part.mesh);if(part.pose)pose_pure(*part.pose);}
        for(const auto* mirror:mesh_mirrors)for(const auto& part:mirror->parts)if(part.arm_publication)arm_publication_final(*part.arm_publication);
        lookup_finish();r->complete=1;diagnostic.complete=true;return 1;
    }catch(const std::exception& e){failure(r,e.what());return -1;}
}
#include "native_capture_impl.h"
const HsmpPresentation provider{12,0,inspect,capture,create,apply,destroy,discard,retire,probe_retirement,forget_retirements,actor_scope,describe_spline,describe_vertex_state,finish_scene_sets,capture_frame};
}
void hsmp_presentation_set_create_log(HsmpPresentationCreateLog logger){create_logger.store(logger);}
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
