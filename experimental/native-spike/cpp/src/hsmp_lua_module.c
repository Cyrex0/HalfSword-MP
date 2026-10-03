/*
 * Option E module DLL (hsmp_lua.dll). All logic lives in the Rust staticlib
 * crates/hsmp_lua_e; hsmp_lua.def exports its `luaopen_hsmp_lua`. This
 * translation unit exists because MSBuild needs at least one source file, and
 * it keeps an explicit reference to the entry point.
 */
struct lua_State;
int luaopen_hsmp_lua(struct lua_State* L);

int (*const hsmp_lua_entry_point_ref)(struct lua_State*) = luaopen_hsmp_lua;
