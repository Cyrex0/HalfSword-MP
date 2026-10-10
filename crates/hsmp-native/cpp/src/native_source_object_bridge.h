// Pinned UE4SS LuaMadeSimple dispatcher supplies the *current* coroutine's
// existing Lua object. This bridge never constructs or retains a Lua context.
#pragma once
#include <cstdint>
#include <cstdio>
#include <string>
#include "lua.hpp"

namespace hsmp_source_object {
using Callback = int (*)(const void*);
using Register = void (*)(const void*, const std::string&, const Callback&);
using GetState = lua_State* (*)(const void*);
using Construct = void (*)(const void*, void*);
inline Register register_function = nullptr;
inline GetState get_state = nullptr;
inline Construct construct = nullptr;
inline char registry_key;
struct Invocation {
    lua_State* state;
    void* object;
    bool entered = false;
    const char* fault = nullptr;
};
inline thread_local Invocation* invocation = nullptr;

inline bool object_userdata(lua_State* L) {
    if (lua_type(L, -1) != LUA_TUSERDATA || !lua_getmetatable(L, -1)) return false;
    luaL_getmetatable(L, "UObject");
    bool same = lua_type(L, -1) == LUA_TTABLE && lua_rawequal(L, -1, -2);
    // The pinned auto_construct_object uses AActor::construct for pawns;
    // ordinary components retain UObject. Accept only these registry identities.
    if (!same) {
        lua_pop(L, 1);
        luaL_getmetatable(L, "AActor");
        same = lua_type(L, -1) == LUA_TTABLE && lua_rawequal(L, -1, -2);
    }
    lua_pop(L, 2);
    return same;
}
inline int factory(const void* context) {
    auto* call = invocation;
    // Even a Lua caller that discovers the registry closure cannot supply an
    // arbitrary pointer. Only the synchronous qualified Rust operation admits it.
    if (!call || !context || !get_state || !construct) return 0;
    if (call->entered) {
        call->fault = "source wrapper factory reentry";
        return 0;
    }
    call->entered = true;
    lua_State* L = get_state(context);
    if (!L || L != call->state) {
        call->fault = "source wrapper current coroutine changed";
        return 0;
    }
    const int top = lua_gettop(L);
    try {
        construct(context, call->object);
        if (lua_gettop(L) != top + 1 || !object_userdata(L)) {
            lua_settop(L, top);
            call->fault = "source wrapper factory userdata/stack contract";
            return 0;
        }
        return 1;
    } catch (...) {
        lua_settop(L, top);
        call->fault = "source wrapper factory exception";
        return 0;
    }
}
inline bool install(const void* context) {
    if (!register_function || !get_state || !construct || !context) return false;
    lua_State* L = get_state(context);
    if (!L) return false;
    const int top = lua_gettop(L);
    try {
        const Callback callback = factory;
        register_function(context, "__HSMPNativeSourceObjectFactory", callback);
        if (lua_getglobal(L, "__HSMPNativeSourceObjectFactory") != LUA_TFUNCTION) {
            lua_settop(L, top);
            return false;
        }
        lua_rawsetp(L, LUA_REGISTRYINDEX, &registry_key);
        lua_pushnil(L);
        lua_setglobal(L, "__HSMPNativeSourceObjectFactory");
        lua_settop(L, top);
        return true;
    } catch (...) {
        lua_settop(L, top);
        return false;
    }
}
inline void remove(lua_State* L) {
    if (!L) return;
    lua_pushnil(L);
    lua_rawsetp(L, LUA_REGISTRYINDEX, &registry_key);
}
inline int protected_factory(lua_State* L) {
    auto* call = invocation;
    if (!call || call->state != L) return 0;
    if (lua_rawgetp(L, LUA_REGISTRYINDEX, &registry_key) != LUA_TFUNCTION) {
        call->fault = "source wrapper factory unavailable";
        return 0;
    }
    // Both the UE4SS dispatch and its postcallbacks, plus the final metatable
    // lookup, stay inside the outer lua_pcall. No Lua error crosses Rust.
    lua_call(L, 0, LUA_MULTRET);
    if (call->fault || !call->entered || lua_gettop(L) != 1 || !object_userdata(L)) {
        call->fault = call->fault ? call->fault : "source wrapper factory userdata/stack contract";
        return 0;
    }
    return 1;
}
inline int32_t push(lua_State* L, uint64_t address, char* reason, uint32_t capacity) {
    const auto fail = [reason, capacity](const char* text) {
        if (reason && capacity) std::snprintf(reason, capacity, "%s", text);
        return int32_t{0};
    };
    if (!L || !address) return fail("source wrapper original object unavailable");
    if (invocation) {
        invocation->fault = "source wrapper factory reentry";
        return fail("source wrapper factory reentry");
    }
    const int top = lua_gettop(L);
    if (!lua_checkstack(L, 8)) return fail("source wrapper stack capacity");
    lua_pushcfunction(L, protected_factory); // zero-upvalue light C function
    Invocation call{L, reinterpret_cast<void*>(address)};
    invocation = &call;
    int status;
    try {
        // Lua errors from UE4SS/allocation/postcallbacks stay inside this C++
        // protected call, never longjmp through the Rust caller.
        status = lua_pcall(L, 0, 1, 0);
    } catch (...) {
        invocation = nullptr;
        lua_settop(L, top);
        return fail("source wrapper protected factory exception");
    }
    invocation = nullptr;
    if (status != LUA_OK || call.fault || !call.entered || lua_gettop(L) != top + 1 || lua_type(L, -1) != LUA_TUSERDATA) {
        lua_settop(L, top);
        return fail(call.fault ? call.fault : "source wrapper protected factory refused");
    }
    return 1;
}
} // namespace hsmp_source_object
