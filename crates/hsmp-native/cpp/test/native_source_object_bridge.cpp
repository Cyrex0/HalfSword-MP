// Actual Lua VM tests of the protected bridge contract. The dispatcher/context
// and remote wrapper are explicit offline stand-ins, not engine parity proof.
#include <cstdio>
#include <cstdlib>
#include <stdexcept>
#include <unordered_map>
#include "native_source_object_bridge.h"

namespace {
struct Context { lua_State* L; };
std::unordered_map<lua_State*, Context> contexts;
hsmp_source_object::Callback registered = nullptr;
lua_State* main_state = nullptr;
lua_State* constructor_state = nullptr;
uint64_t constructor_address = 0;
int calls = 0, checks = 0;
enum Mode { Normal, WrongState, Table, Light, WrongMeta, Zero, Two, LuaError, Exception, Reentry, PostWrongMeta, PostError };
Mode mode = Normal;
void check(bool value, const char* why) {
    ++checks;
    if (!value) { std::fprintf(stderr, "source object bridge FAIL: %s\n", why); std::exit(1); }
}
lua_State* state(const void* context) { return static_cast<const Context*>(context)->L; }
int dispatch(lua_State* L) {
    auto it = contexts.find(L);
    if (it == contexts.end()) return luaL_error(L, "offline dispatcher unknown coroutine");
    const Context* c = mode == WrongState ? &contexts.at(main_state) : &it->second;
    const int count = registered(c);
    if (mode == PostError) return luaL_error(L, "offline postcallback error");
    if (mode == PostWrongMeta && count == 1) {
        luaL_getmetatable(L, "Other");
        lua_setmetatable(L, -2);
    }
    return count;
}
void registration(const void* context, const std::string& name, const hsmp_source_object::Callback& callback) {
    registered = callback;
    lua_State* L = state(context);
    lua_pushcfunction(L, dispatch);
    lua_setglobal(L, name.c_str());
}
void constructor(const void* context, void* address) {
    ++calls;
    lua_State* L = state(context);
    constructor_state = L;
    constructor_address = reinterpret_cast<uint64_t>(address);
    switch (mode) {
    case Table: lua_newtable(L); return;
    case Light: lua_pushlightuserdata(L, address); return;
    case Zero: return;
    case Two: lua_pushinteger(L, 1); lua_pushinteger(L, 2); return;
    case LuaError: luaL_error(L, "offline allocation callback error"); return;
    case Exception: throw std::runtime_error("offline constructor exception");
    case Reentry: {
        char reason[128]{};
        check(hsmp_source_object::push(L, constructor_address, reason, sizeof reason) == 0, "nested factory refuses");
        break;
    }
    default: break;
    }
    auto* copy = static_cast<uint64_t*>(lua_newuserdatauv(L, sizeof(uint64_t), 0));
    *copy = constructor_address;
    luaL_getmetatable(L, mode == WrongMeta ? "Other" : "UObject");
    lua_setmetatable(L, -2);
}
void metatables(lua_State* L) {
    luaL_newmetatable(L, "UObject"); lua_pop(L, 1);
    luaL_newmetatable(L, "Other"); lua_pop(L, 1);
}
void attempt(lua_State* L, Mode wanted, bool success) {
    mode = wanted;
    const int before = lua_gettop(L);
    char reason[192]{};
    const int result = hsmp_source_object::push(L, 0x12345678, reason, sizeof reason);
    check(result == (success ? 1 : 0), "factory result");
    check(lua_gettop(L) == before + (success ? 1 : 0), "exact stack restoration/result");
    check(hsmp_source_object::invocation == nullptr, "no retained TLS context/object");
    if (success) {
        check(lua_type(L, -1) == LUA_TUSERDATA, "full userdata result");
        check(*static_cast<uint64_t*>(lua_touserdata(L, -1)) == 0x12345678, "original scalar copied by factory");
        check(constructor_state == L, "constructor uses current coroutine stack");
        lua_pop(L, 1);
    } else check(reason[0] != 0, "bounded explicit failure reason");
}
}
int main() {
    main_state = luaL_newstate();
    contexts.emplace(main_state, Context{main_state});
    metatables(main_state);
    hsmp_source_object::register_function = registration;
    hsmp_source_object::get_state = state;
    hsmp_source_object::construct = constructor;
    check(hsmp_source_object::install(&contexts.at(main_state)), "factory installed from opaque actual context");
    check(lua_getglobal(main_state, "__HSMPNativeSourceObjectFactory") == LUA_TNIL, "temporary global removed");
    lua_pop(main_state, 1);
    lua_pushinteger(main_state, 71);
    attempt(main_state, Normal, true);
    for (Mode bad : {Table, Light, WrongMeta, Zero, Two, LuaError, Exception, Reentry, PostWrongMeta, PostError}) {
        attempt(main_state, bad, false);
        attempt(main_state, Normal, true);
    }
    // Registry is shared, but the dispatcher chooses the calling thread's
    // original context, never the root context used at install time.
    lua_State* coroutine = lua_newthread(main_state);
    const int main_top = lua_gettop(main_state);
    contexts.emplace(coroutine, Context{coroutine});
    lua_pushinteger(coroutine, 99);
    attempt(coroutine, Normal, true);
    check(lua_gettop(main_state) == main_top, "root stack untouched by coroutine wrapper");
    const int before_wrong_state = calls;
    attempt(coroutine, WrongState, false);
    check(calls == before_wrong_state, "mismatched context never constructs");
    contexts.erase(coroutine);
    attempt(coroutine, Normal, false);
    contexts.emplace(coroutine, Context{coroutine});
    attempt(coroutine, Normal, true);

    // A discovered closure has no object argument route outside the one-shot
    // original native admission. Passing any scalar does not construct.
    const int before_unadmitted = calls;
    const int top = lua_gettop(coroutine);
    lua_rawgetp(coroutine, LUA_REGISTRYINDEX, &hsmp_source_object::registry_key);
    lua_pushinteger(coroutine, 0x12345678);
    check(lua_pcall(coroutine, 1, LUA_MULTRET, 0) == LUA_OK, "unadmitted closure bounded refusal");
    check(lua_gettop(coroutine) == top && calls == before_unadmitted, "no arbitrary address constructor");
    hsmp_source_object::remove(main_state);
    attempt(coroutine, Normal, false);
    contexts.clear();
    lua_close(main_state);
    std::printf("source object bridge: %d checks PASS\n", checks);
}
