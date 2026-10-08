// Runs the actual Lua API and pairing state against a bounded fake provider; no game/DLL execution.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include "lua.hpp"
#include "box_snapshot_probe.h"
namespace
{
using namespace hsmp_box;
unsigned checks{}, reads{}, keys{}, enrolls{}, submissions{}, validations{};
bool good_thread=true, bad_enrollment{}, bad_snapshot{}, serial_zero{};
bool malformed_costs{};
bool bad_prepared{};
lua_State* stop_during_validation{};
lua_State* reenter_during_enrollment{};
EnrollmentTiming measured{};
const EnrollmentTiming* enrollment_timing() { return &measured; }
const char* diagnostic{};
const char* enrollment_detail() { return diagnostic; }
std::uint64_t time_ms=1000;
Sink callback{};
Scope scoped{};
Identity id(std::uint64_t a)
{ return {a,(serial_zero ? 0ull : 1ull<<32)|a,a+100,a+200,(1ull<<32)|(a+200),a+300}; }
Scope sample_scope()
{ return {id(1),id(2),id(3),id(4),id(5),123456789,1,1}; }
struct Frame { unsigned role{}; std::uint64_t token{}; double x{6},y{1},z{10}; bool wrong_box{}; };
bool on_thread() { return good_thread; }
std::uint64_t now() { return time_ms; }
bool enroll(const Enrollment& e,Scope& out,Reason& why)
{
    ++enrolls;
    if (reenter_during_enrollment && luaL_dostring(reenter_during_enrollment,
        "assert(N.stop()); local ok,why=N.prepare(a); assert(ok==nil and why=='observer setup busy'); assert(N.begin(a)==nil and N.activate(1,a)==nil)")!=LUA_OK) std::exit(1);
    measured.begin(); measured.stage("initialize",1000); measured.stage("scope_identity",1004); measured.finish(1009);
    if (malformed_costs) measured.count=99;
    if (bad_enrollment || e.pawn.address!=2 || !e.pawn.path || std::wcscmp(e.pawn.path,L"/pawn")!=0)
    { why=Reason::Identity; return false; }
    out=sample_scope(); out.match_id=e.match_id; out.round=e.round; out.life=e.life; scoped=out; return true;
}
bool submit(Sink s,Reason&) { callback=s; ++submissions; return true; }
bool key(void* context,void* frame,Key& out,Reason&)
{
    ++keys;
    const auto& f=*static_cast<Frame*>(frame);
    if (!f.role || reinterpret_cast<std::uint64_t>(context)!=2) return false;
    out={f.token,1000+f.role,2,f.role}; return true;
}
bool snapshot(const Key& k,void* frame,Snapshot& out,Reason& why)
{
    ++reads; const auto& f=*static_cast<Frame*>(frame);
    if (bad_snapshot) { why=Reason::Identity; return false; }
    if (f.wrong_box) { why=Reason::Params; return false; } // Foreign formal never dereferenced.
    out={scoped,id(k.node),{f.x,f.y,f.z},true,Reason::None}; return true;
}
bool prepared_current(const Scope& expected,Reason& why)
{
    ++validations;
    if (stop_during_validation && luaL_dostring(stop_during_validation,
        "assert(N.stop()); local ok,why=N.prepare(a); assert(ok==nil and why=='observer setup busy'); assert(N.begin(a)==nil and N.activate(1,a)==nil)")!=LUA_OK) std::exit(1);
    if (bad_prepared) { why=Reason::Params; return false; }
    if (expected!=scoped) { why=Reason::Identity; return false; }
    return true;
}
const Provider fake{on_thread,now,enroll,submit,key,snapshot,enrollment_detail,enrollment_timing,prepared_current};
void check(bool ok,const char* message)
{ ++checks; if (!ok) { std::fprintf(stderr,"FAIL %s\n",message); std::exit(1); } }
void lua(lua_State* L,const char* code)
{
    if (luaL_dostring(L,code)!=LUA_OK)
    { std::fprintf(stderr,"Lua fixture: %s\n",lua_tostring(L,-1)); std::exit(1); }
    ++checks;
}
void event(unsigned phase,Frame& f)
{ check(callback!=nullptr,"observer submitted"); callback(phase,reinterpret_cast<void*>(2),&f); }
const char* start=R"(
 a={match_id=123456789,round=1,life=1,duration_ms=15000,calls=32}
 for _,k in ipairs({'world','pawn','mesh','box','box_owner'}) do
   local n=({world=1,pawn=2,mesh=3,box=4,box_owner=5})[k]
   a[k]={path='/'..k,address=n}
 end
 assert(N.begin(a)==true)
)";
const char* mark1=R"(
 assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,
   round=1,life=1,role=1,marker=777})==true)
)";
}
const hsmp_box::Provider& hsmp_reflect_box_provider() { return fake; }
int main()
{
    using namespace hsmp_box;
    EnrollmentTiming timing{}; timing.stage("off",10); timing.finish(11);
    check(timing.count==0 && !timing.active(),"timing is off before explicit enrollment");
    timing.begin(); timing.stage("first",10); timing.stage("second",14); timing.finish(19);
    check(timing.count==2 && timing.rows[0].end_ms-timing.rows[0].start_ms==4
        && timing.rows[1].end_ms-timing.rows[1].start_ms==5 && !timing.active(),"stage transition and failure finish close exact elapsed values");
    timing.begin(); timing.stage("backwards",20); timing.finish(19);
    check(!timing.rows[0].available,"backwards clock remains unavailable");
    timing.begin(); for (unsigned i=0;i<17;++i) timing.stage("bounded",i); timing.finish(99);
    check(timing.count==16 && timing.overflow && !timing.active() && timing.rows[15].end_ms==16,"fixed stage limit does not overwrite or reopen rows");
    timing.begin(); check(timing.count==0 && !timing.overflow,"next explicit enrollment resets stage costs only");
    check(input_bounds(8,8,16,true,true,false),"exact in-bounds formal ObjectProperty");
    check(!input_bounds(12,8,16,true,true,false),"formal overrun unavailable");
    check(!input_bounds(-1,8,16,true,true,false),"negative formal offset unavailable");
    check(!input_bounds(0,8,16,false,true,false),"wrong native property type unavailable");
    check(!input_bounds(0,8,16,true,false,false),"same-name local is not a formal input");
    check(!input_bounds(0,8,16,true,true,true),"reference/output formal unavailable");
    check(!input_bounds(0,4,16,true,true,false),"pointer size mismatch unavailable");
    auto class_zero=id(2); class_zero.class_weak=202;
    check(!class_zero.persistent(),"serial-zero class descriptor cannot qualify full lifetime proof");
    auto negative=id(2); negative.weak=(0xffffffffull<<32)|2;
    check(!negative.persistent(),"encoded negative object serial never qualifies");
    negative=id(2); negative.class_weak=(0xffffffffull<<32)|202;
    check(!negative.persistent(),"encoded negative class serial never qualifies");
    lua_State* L=luaL_newstate(); luaL_openlibs(L);
    lua_newtable(L); lua_setglobal(L,"HSMPNative");
    hsmp_box_probe_install(L); check(lua_gettop(L)==0,"installer balances stack");
    lua(L,"N=HSMPNative.box_probe; assert(N.status().active==false and N.status().submitted==false and N.status().enrollment_costs==nil); real_print=print; cost_logs={}; print=function(s) if s:find('BOXOBS_ENROLL_COST',1,true) then cost_logs[#cost_logs+1]=s end end");
    check(keys==0 && reads==0 && enrolls==0,"default OFF makes no engine/frame reads");
    SetEnvironmentVariableA("HSMP_DEV",nullptr);
    lua(L,"assert(N.begin({})==nil)"); check(enrolls==0,"production activation refused before enrollment");
    SetEnvironmentVariableA("HSMP_DEV","1"); SetEnvironmentVariableA("HSMP_INST","bad"); lua(L,start);
    lua(L,"local ok,c=N.begin(a); assert(ok==true and c.clock=='GetTickCount64' and not c.authority and c.count==2 and #c.rows==2 and c.rows[1].elapsed_ms==4 and c.rows[2].elapsed_ms==5 and cost_logs[#cost_logs]:find('inst=unknown',1,true))");
    SetEnvironmentVariableA("HSMP_INST","2"); malformed_costs=true;
    lua(L,"local ok,c=N.begin(a); assert(ok==true and c.count==16 and c.overflow and #c.rows==16 and cost_logs[#cost_logs]:find('inst=2',1,true) and #cost_logs[#cost_logs]<2048)");
    malformed_costs=false;
    lua(L,"local saved=print; print=function() error('optional print failed') end; local ok,c=N.begin(a); print=saved; assert(ok==true and c.rows[1].elapsed_ms==4 and N.status().active)");
    lua(L,"local saved=print; print=function() assert(N.stop()==true) end; local ok,c=N.begin(a); print=saved; assert(ok==true and c.rows[2].elapsed_ms==5 and not N.status().active)");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok==true and math.type(ticket)=='integer' and ticket>0 and not N.status().active)");
    Frame prep_frame{1,90}; unsigned prep_reads=reads; event(1,prep_frame);
    check(reads==prep_reads,"prepared observer never reads native frames before activation");
    const auto prep_enrolls=enrolls;
    lua(L,"assert(N.activate(ticket,a)==true and N.status().active); assert(N.activate(ticket,a)==nil)");
    check(enrolls==prep_enrolls && validations==1,"activation revalidates without repeating path enrollment");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok); assert(N.stop()); assert(N.activate(ticket,a)==nil and not N.status().active)");
    lua(L,"local saved=print; print=function() assert(N.stop()) end; local ok; ok,ticket=N.prepare(a); print=saved; assert(ok and N.activate(ticket,a)==nil and not N.status().active)");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok)"); time_ms+=251;
    lua(L,"local ok,why=N.activate(ticket,a); assert(ok==nil and why=='observer preparation expired' and not N.status().active)");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok); a.box.address=99; assert(N.activate(ticket,a)==nil); a.box.address=4; assert(N.activate(ticket,a)==nil)");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok)"); bad_prepared=true;
    lua(L,"local ok,why=N.activate(ticket,a); assert(ok==nil and why=='formal parameters unavailable' and not N.status().active)"); bad_prepared=false;
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok)"); stop_during_validation=L;
    unsigned busy_enrolls=enrolls;
    lua(L,"assert(N.activate(ticket,a)==nil and not N.status().active)"); stop_during_validation=nullptr;
    check(enrolls==busy_enrolls,"stop during activation cannot allow nested setup to overwrite provider metadata");
    reenter_during_enrollment=L;busy_enrolls=enrolls;
    lua(L,"assert(N.prepare(a)==nil and not N.status().active)"); reenter_during_enrollment=nullptr;
    check(enrolls==busy_enrolls+1,"stop during enrollment retains in-flight guard against nested provider mutation");
    lua(L,"local ok; ok,ticket=N.prepare(a); assert(ok)");
    lua_State* prepared_foreign=luaL_newstate();luaL_openlibs(prepared_foreign);lua_newtable(prepared_foreign);lua_setglobal(prepared_foreign,"HSMPNative");hsmp_box_probe_install(prepared_foreign);
    lua(prepared_foreign,"N=HSMPNative.box_probe; assert(N.prepare({})==nil and N.begin({})==nil and N.activate(1,{})==nil and N.stop()==nil)");
    lua_close(prepared_foreign); time_ms+=250;
    lua(L,"assert(N.activate(ticket,a)==true and N.status().active)");
    lua(L,start);
    Frame outer{1,10},inner{2,11,20,1,25}; event(1,outer); event(1,inner);
    lua(L,"assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=2,marker=778}))");
    inner.z=31; event(2,inner); lua(L,mark1); outer.x=20; outer.z=31; event(2,outer);
    lua(L,R"(r=N.read(); assert(#r==2 and r[1].parent==r[2].id and r[1].depth==1)
      assert(r[2].before[1]==6 and r[1].before[1]==20 and r[2].after[1]==20)
      assert(r[1].qualified and r[2].qualified and r[2].lua_inside and r[2].marker==777)
      assert(r[2].authority==false and r[2].context_declared and r[2].lifetime_available)
      assert(#N.read()==0))");
    lua(L,"assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=1,marker=0})==nil)");
    // A late Lua POST cannot retrospectively qualify the completed call.
    event(1,outer); event(2,outer);
    lua(L,"r=N.read(); assert(#r==1 and not r[1].qualified and not r[1].lua_inside)");
    serial_zero=true; lua(L,start); event(1,outer); lua(L,mark1); event(2,outer);
    lua(L,"r=N.read(); assert(#r==1 and r[1].pre_available and r[1].post_available and not r[1].qualified and not r[1].lifetime_available and r[1].reason=='lifetime unavailable')");
    serial_zero=false; lua(L,start); event(1,outer); lua(L,mark1); bad_snapshot=true; event(2,outer); bad_snapshot=false;
    lua(L,"r=N.read(); assert(#r==1 and not r[1].qualified and r[1].after==nil and r[1].reason=='identity unavailable')");
    lua(L,start); outer.wrong_box=true; event(1,outer); event(2,outer); outer.wrong_box=false;
    lua(L,"r=N.read(); assert(#r==1 and not r[1].pre_available and r[1].before==nil and not r[1].qualified)");
    lua(L,start); event(1,outer); event(1,inner); event(2,outer);
    lua(L,"assert(not N.status().active and N.status().discarded==2 and #N.read()==0)");
    lua(L,start); event(1,outer); time_ms+=15000; event(2,outer);
    lua(L,"assert(not N.status().active and N.status().reason=='expired' and #N.read()==0)");
    time_ms+=1; lua(L,start);
    Frame other{}; unsigned previous=reads; event(1,other); check(reads==previous,"non-target function never snapshots");
    lua(L,"a.calls=1; assert(N.begin(a))"); event(1,outer); event(1,inner); event(2,inner); lua(L,mark1); event(2,outer);
    check(reads==previous+2,"fixed budget excludes nested extra capture/read");
    lua(L,"r=N.read(); assert(#r==1 and r[1].qualified and not N.status().active)");
    // Counterexample 1: ignored same-role nested PRE must not let child Lua POST mark its ancestor.
    lua(L,start); lua(L,"a.calls=1;assert(N.begin(a))");
    Frame recursive{1,12}; event(1,outer); event(1,recursive);
    lua(L,"assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=1,marker=0})==nil and #N.read()==0 and not N.status().active)");
    // Counterexample 2: child native POST before its Lua POST must not reopen an ancestor lookup.
    lua(L,start); event(1,outer); event(1,recursive); event(2,recursive);
    lua(L,"assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=1,marker=0})==nil and #N.read()==0)");
    event(2,outer);
    lua(L,start); event(1,outer);
    lua_State* foreign=luaL_newstate();luaL_openlibs(foreign);lua_newtable(foreign);lua_setglobal(foreign,"HSMPNative");hsmp_box_probe_install(foreign);
    lua(foreign,"N=HSMPNative.box_probe; assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=1,marker=0})==nil and N.stop()==nil and N.read()==nil)");
    lua_close(foreign);
    lua(L,"local co=coroutine.create(function() assert(N.mark({world=1,pawn=2,mesh=3,box=4,box_owner=5,match_id=123456789,round=1,life=1,role=1,marker=0})) end); assert(coroutine.resume(co))");
    event(2,outer);lua(L,"r=N.read();assert(#r==1 and r[1].qualified)");
    lua(L,start); Frame depth[9]; for (unsigned i=0;i<9;++i) { depth[i]={1,100+i}; event(1,depth[i]); }
    lua(L,"assert(not N.status().active and N.status().reason=='unpaired ordering' and #N.read()==0)");
    lua(L,start); event(1,outer); event(1,outer);
    lua(L,"assert(not N.status().active and N.status().reason=='unpaired ordering')");
    lua(L,start); event(1,outer); good_thread=false; previous=keys; event(2,outer);
    check(keys==previous,"foreign thread callback never reads FFrame"); good_thread=true;
    lua(L,"assert(not N.status().active and N.status().reason=='game thread unavailable' and #N.read()==0)");
    lua(L,start); bad_enrollment=true; lua(L,"assert(N.begin(a)==nil and not N.status().active)");
    diagnostic="stage=gd_hit_box failure=children_limit children=1070";
    previous=submissions;
    lua(L,"local ok,why=N.begin(a); assert(ok==nil and why=='identity unavailable [stage=gd_hit_box failure=children_limit children=1070]' and not N.status().active)");
    check(submissions==previous,"failed enrollment diagnostics never submit callbacks");
    const std::string long_detail(1024,'x'); diagnostic=long_detail.c_str();
    lua(L,"local ok,why=N.begin(a); assert(ok==nil and #why==#'identity unavailable ['+512+1)");
    diagnostic=nullptr;
    lua(L,"local ok,why=N.begin(a); assert(ok==nil and why=='identity unavailable')");
    bad_enrollment=false;
    lua(L,"a.duration_ms=15001; assert(N.begin(a)==nil); a.duration_ms=15000; a.calls=33; assert(N.begin(a)==nil)");
    lua(L,"a.calls=32; a.pawn.address=1.5; assert(N.begin(a)==nil); a.pawn.address=2; a.pawn.path='/pawn'..string.char(0)..'x'; assert(N.begin(a)==nil)");
    lua(L,start); lua(L,"assert(N.stop()==true and not N.status().active)");
    previous=keys; event(1,outer); check(keys==previous,"stopped observer skips all frame reads");
    lua(L,"for i=1,40 do assert(N.begin(a)==true) end; assert(#cost_logs<=32); local n=#cost_logs; assert(N.begin(a)==true and #cost_logs==n); assert(N.stop()==true); print=real_print");
    lua_close(L); SetEnvironmentVariableA("HSMP_DEV",nullptr);
    std::printf("box_snapshot_probe: %u checks passed (actual Lua API, mock provider)\n",checks);
}
