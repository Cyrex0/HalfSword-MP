// C ABI of the Rust staticlib crates/hsmp-native (hsmp_native.lib).
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

struct lua_State;

// Option F: register the global table `HSMPNative` into `L` (called from on_lua_start before
// the mod's main.lua runs). `mod_name` may be NULL. Returns 1 on success, 0 on failure.
// Never raises; leaves the stack balanced.
int hsmp_native_open(struct lua_State* L, const char* mod_name);

// Option E entry point (`require "hsmp_lua"` / package.loadlib). Returns the same table as F
// when F already registered `HSMPNative` in this state.
int luaopen_hsmp_lua(struct lua_State* L);

// hsmp_luauser.c: how lua_lock is resolved ("host ..." or "private ...").
const char* hsmp_lua_lock_mode(void);

#ifdef __cplusplus
}
#endif
