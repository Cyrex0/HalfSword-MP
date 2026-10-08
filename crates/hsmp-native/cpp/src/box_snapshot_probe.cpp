// Lua control/lookup for a default-OFF proof-only observer. No engine writes, files or logging.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <string>
#include <limits>
#include <atomic>
#include "lua.hpp"
#include "box_snapshot_probe.h"
namespace
{
using namespace hsmp_box;
State state;
lua_State* owner_state{};
bool submitted{};
std::atomic<bool> enabled{};
std::atomic<unsigned> deferred_failure{};
const Provider& provider() { return hsmp_reflect_box_provider(); }
lua_State* vm(lua_State* L)
{
    lua_rawgeti(L,LUA_REGISTRYINDEX,LUA_RIDX_MAINTHREAD);
    auto* main=lua_tothread(L,-1); lua_pop(L,1); return main;
}
void synchronize()
{
    const auto failure=deferred_failure.exchange(0);
    if (failure) state.stop(static_cast<Reason>(failure));
}
void disable(Reason why=Reason::Off)
{ enabled.store(false); state.stop(why); }
void rawfield(lua_State* L,int table,const char* key)
{ lua_pushstring(L,key); lua_rawget(L,table); }
bool integer(lua_State* L,int table,const char* key,std::uint64_t& out,bool zero=false)
{
    rawfield(L,table,key);
    const bool typed=lua_isinteger(L,-1)!=0;
    const auto n=typed ? lua_tointeger(L,-1) : -1;
    lua_pop(L,1);
    if (!typed || n<0 || (!zero && n==0)) return false;
    out=static_cast<std::uint64_t>(n); return true;
}
bool object(lua_State* L,int table,const char* key,ObjectInput& out,std::wstring& path)
{
    rawfield(L,table,key);
    if (!lua_istable(L,-1)) { lua_pop(L,1); return false; }
    const int t=lua_absindex(L,-1);
    bool ok=integer(L,t,"address",out.address);
    rawfield(L,t,"path"); std::size_t n{};
    const char* s=lua_type(L,-1)==LUA_TSTRING ? lua_tolstring(L,-1,&n) : nullptr;
    if (!s || n==0 || n>2048 || std::string(s,n).find('\0')!=std::string::npos) ok=false;
    if (ok)
    {
        const auto count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,static_cast<int>(n),nullptr,0);
        if (count<=0) ok=false;
        else { path.resize(static_cast<std::size_t>(count));
            ok=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,static_cast<int>(n),path.data(),count)==count; }
    }
    lua_pop(L,2); out.path=path.c_str(); return ok;
}
int unavailable(lua_State* L,const char* why)
{ lua_pushnil(L); lua_pushstring(L,why); return 2; }
void number(lua_State* L,const char* key,std::uint64_t value)
{ lua_pushinteger(L,static_cast<lua_Integer>(value)); lua_setfield(L,-2,key); }
void boolean(lua_State* L,const char* key,bool value)
{ lua_pushboolean(L,value); lua_setfield(L,-2,key); }
void text(lua_State* L,const char* key,const char* value)
{ lua_pushstring(L,value); lua_setfield(L,-2,key); }
void identity(lua_State* L,const char* key,const Identity& id)
{
    lua_createtable(L,0,7); number(L,"address",id.address); number(L,"weak",id.weak); number(L,"name",id.name);
    number(L,"class_address",id.class_address); number(L,"class_weak",id.class_weak); number(L,"class_name",id.class_name);
    boolean(L,"persistent",id.persistent()); lua_setfield(L,-2,key);
}
void scope(lua_State* L,const Scope& s)
{
    number(L,"match_id",s.match_id); number(L,"round",s.round); number(L,"life",s.life);
    identity(L,"world",s.world); identity(L,"pawn",s.pawn); identity(L,"mesh",s.mesh);
    identity(L,"box",s.box); identity(L,"box_owner",s.box_owner);
}
void extent(lua_State* L,const char* key,const Snapshot& s)
{
    if (!s.available) { lua_pushnil(L); lua_setfield(L,-2,key); return; }
    lua_createtable(L,3,0);
    const double values[]={s.extent.x,s.extent.y,s.extent.z};
    for (int i=0;i<3;++i) { lua_pushnumber(L,values[i]); lua_rawseti(L,-2,i+1); }
    lua_setfield(L,-2,key);
}
void observe(unsigned phase,void* context,void* frame)
{
    if (!enabled.load()) return;
    // A foreign callback must never read/write the game-thread state or touch FFrame/UObjects.
    if (!provider().on_thread())
    { deferred_failure.store(static_cast<unsigned>(Reason::Thread)); enabled.store(false); return; }
    synchronize();
    const auto now=provider().now_ms(); state.poll(now);
    if (!state.active) { enabled.store(false); return; }
    try
    {
        Key key{}; Reason why=Reason::None;
        if (!provider().key(context,frame,key,why))
        { if (why!=Reason::None) disable(why); return; }
        if (phase==1 && !state.guard_entry(key,now))
        { if (!state.active) enabled.store(false); return; }
        if (phase==2 && !state.has(key))
        { state.post(key,{},now); if (!state.active) enabled.store(false); return; }
        Snapshot s{};
        if (!provider().snapshot(key,frame,s,why)) { s.available=false; s.reason=why; }
        if (phase==1) state.pre(key,s,now); else if (phase==2) state.post(key,s,now);
        if (!state.active) enabled.store(false);
    }
    catch (...) { disable(Reason::Unavailable); }
}
int begin_impl(lua_State* L)
{
    char dev[8]{};
    if (GetEnvironmentVariableA("HSMP_DEV",dev,sizeof dev)!=1 || dev[0]!='1') return unavailable(L,"developer mode required");
    if (!provider().on_thread()) return unavailable(L,reason_name(Reason::Thread));
    synchronize();
    if (state.active && owner_state!=vm(L)) return unavailable(L,"another developer state owns the observer");
    disable(Reason::Scope);
    if (!lua_istable(L,1)) return unavailable(L,"enrollment table required");
    Enrollment input{}; std::wstring paths[5]; std::uint64_t round{},life{},duration{},limit{};
    if (!integer(L,1,"match_id",input.match_id) || !integer(L,1,"round",round) || !integer(L,1,"life",life)
        || round>UINT32_MAX || life>UINT32_MAX || !integer(L,1,"duration_ms",duration)
        || duration>kMaxDurationMs || !integer(L,1,"calls",limit) || limit>kMaxPairs
        || !object(L,1,"world",input.world,paths[0]) || !object(L,1,"pawn",input.pawn,paths[1])
        || !object(L,1,"mesh",input.mesh,paths[2]) || !object(L,1,"box",input.box,paths[3])
        || !object(L,1,"box_owner",input.box_owner,paths[4])) return unavailable(L,reason_name(Reason::BadInput));
    input.round=static_cast<std::uint32_t>(round); input.life=static_cast<std::uint32_t>(life);
    Scope enrolled{}; Reason why=Reason::Unavailable;
    // Re-enrollment must never leave an older binding active after a failed path/identity check.
    if (!provider().enroll(input,enrolled,why) || !provider().submit(observe,why)) return unavailable(L,reason_name(why));
    submitted=true;
    if (!state.begin(enrolled,provider().now_ms(),duration,static_cast<unsigned>(limit)))
        return unavailable(L,reason_name(Reason::BadInput));
    owner_state=vm(L); enabled.store(true); lua_pushboolean(L,1); return 1;
}
int begin(lua_State* L)
{ try { return begin_impl(L); } catch (...) { disable(Reason::Unavailable); return unavailable(L,"observer unavailable"); } }
int stop(lua_State* L)
{
    if (!provider().on_thread()) return unavailable(L,reason_name(Reason::Thread));
    synchronize();
    if (owner_state && owner_state!=vm(L)) return unavailable(L,"observer owner required");
    disable(); lua_pushboolean(L,1); return 1;
}
int status(lua_State* L)
{
    if (!provider().on_thread()) return unavailable(L,reason_name(Reason::Thread));
    synchronize();
    state.poll(provider().now_ms()); lua_createtable(L,0,13);
    boolean(L,"active",state.active); boolean(L,"submitted",submitted); boolean(L,"authority",false);
    boolean(L,"qualified",false); text(L,"reason",reason_name(state.last)); number(L,"deadline_ms",state.deadline);
    number(L,"entries",state.entries); number(L,"completed",state.completed()); number(L,"pending",state.pending());
    number(L,"pending_role",state.pending_role());
    number(L,"unmatched",state.unmatched); number(L,"discarded",state.discarded); number(L,"marks_outside",state.marks_outside);
    number(L,"max_pairs",kMaxPairs); number(L,"max_depth",kMaxDepth); return 1;
}
int read(lua_State* L)
{
    if (!provider().on_thread()) return unavailable(L,reason_name(Reason::Thread));
    synchronize();
    if (owner_state && owner_state!=vm(L)) return unavailable(L,"observer owner required");
    state.poll(provider().now_ms()); lua_createtable(L,static_cast<int>(state.completed()),0);
    for (unsigned i=0;i<state.completed();++i)
    {
        const auto& p=state.at(i); lua_createtable(L,0,25);
        number(L,"id",p.id); number(L,"parent",p.parent); number(L,"depth",p.depth); number(L,"role",p.key.role);
        number(L,"pre_ms",p.pre_ms); number(L,"post_ms",p.post_ms); number(L,"lua_marks",p.lua_marks); number(L,"marker",p.marker);
        boolean(L,"qualified",p.qualified); boolean(L,"authority",false); boolean(L,"lua_inside",p.lua_inside);
        boolean(L,"paired",true);
        boolean(L,"pre_available",p.before.available); boolean(L,"post_available",p.after.available);
        boolean(L,"lifetime_available",p.before.persistent() && p.after.persistent());
        boolean(L,"context_declared",true); text(L,"reason",reason_name(p.reason));
        scope(L,p.before.available ? p.before.scope : state.scope);
        identity(L,"function",p.before.function); extent(L,"before",p.before); extent(L,"after",p.after);
        lua_rawseti(L,-2,static_cast<lua_Integer>(i+1));
    }
    state.drain(); return 1;
}
int mark(lua_State* L)
{
    if (!provider().on_thread()) return unavailable(L,reason_name(Reason::Thread));
    synchronize();
    if (owner_state && owner_state!=vm(L)) return unavailable(L,"observer owner required");
    if (!lua_istable(L,1)) return unavailable(L,reason_name(Reason::BadInput));
    Scope fresh=state.scope; std::uint64_t values[9]{};
    const char* keys[]={"world","pawn","mesh","box","box_owner","match_id","round","life","role"};
    for (unsigned i=0;i<9;++i) if (!integer(L,1,keys[i],values[i])) return unavailable(L,reason_name(Reason::BadInput));
    if (values[6]>UINT32_MAX || values[7]>UINT32_MAX || values[8]>2) return unavailable(L,reason_name(Reason::BadInput));
    fresh.world.address=values[0]; fresh.pawn.address=values[1]; fresh.mesh.address=values[2];
    fresh.box.address=values[3]; fresh.box_owner.address=values[4]; fresh.match_id=values[5];
    fresh.round=static_cast<std::uint32_t>(values[6]); fresh.life=static_cast<std::uint32_t>(values[7]);
    std::uint64_t marker{}; if (!integer(L,1,"marker",marker,true)) return unavailable(L,reason_name(Reason::BadInput));
    const auto ok=state.mark(fresh,static_cast<unsigned>(values[8]),marker,provider().now_ms());
    if (!ok) return unavailable(L,"no active exact entry");
    lua_pushboolean(L,1); return 1; // No unpaired entry geometry is returned.
}
}
void hsmp_box_probe_install(lua_State* L)
{
    const auto top=lua_gettop(L);
    if (lua_getglobal(L,"HSMPNative")==LUA_TTABLE)
    {
        lua_createtable(L,0,5);
        struct Entry { const char* name; lua_CFunction fn; };
        for (const auto& e : {Entry{"begin",begin},Entry{"stop",stop},Entry{"status",status},Entry{"read",read},Entry{"mark",mark}})
        { lua_pushcfunction(L,e.fn); lua_setfield(L,-2,e.name); }
        lua_setfield(L,-2,"box_probe");
    }
    lua_settop(L,top);
}
