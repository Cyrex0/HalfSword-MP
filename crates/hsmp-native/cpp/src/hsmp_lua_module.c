/*
 * Option E module DLL (hsmp_lua.dll). All logic lives in the Rust staticlib
 * crates/hsmp-native; hsmp_lua.def exports its `luaopen_hsmp_lua`. This translation unit
 * exists because MSBuild needs at least one source file, and it keeps an explicit reference
 * to the entry point.
 *
 * Load it from ONE absolute path for the whole process (the facade uses
 * package.loadlib("<Mods>/HSMPNative/dlls/hsmp_lua.dll", "luaopen_hsmp_lua")): every
 * distinct path is a distinct DLL instance with its own process-global IPC state.
 */
struct lua_State;
int luaopen_hsmp_lua(struct lua_State* L);

int (*const hsmp_lua_entry_point_ref)(struct lua_State*) = luaopen_hsmp_lua;
