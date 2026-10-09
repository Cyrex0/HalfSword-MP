// C ABI of the Rust staticlib crates/hsmp-native (hsmp_native.lib).
#pragma once
#include <stdint.h>

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

// Diagnostic hooks: only a thread already established by Native::frame().
// Unknown thread, busy/poisoned state and another thread all return 0.
int hsmp_native_caller_thread_ok(void);
enum HsmpCallerAdmission {
    HSMP_CALLER_ALLOWED=0, HSMP_CALLER_WOULD_BLOCK=1, HSMP_CALLER_MUTEX_POISONED=2,
    HSMP_CALLER_NATIVE_POISONED=3, HSMP_CALLER_FRAME_THREAD_UNSET=4,
    HSMP_CALLER_WRONG_THREAD=5, HSMP_CALLER_PANIC=6
};
// Precise result from one try-lock attempt; the old boolean is result==ALLOWED.
int hsmp_native_caller_admission(void);

// hsmp_luauser.c: how lua_lock is resolved ("host ..." or "private ...").
const char* hsmp_lua_lock_mode(void);

// Rare static spline/profile checkpoints only. This never calls Native's mutex
// admission helpers and never dereferences an engine object.
struct HsmpNativeProfileTrace {
    uint64_t epoch,handle,guards,admissions,finds,events,elapsed_us;
    uint32_t entity,incarnation,dir_seq,seq;
};
#ifdef __cplusplus
static_assert(sizeof(HsmpNativeProfileTrace)==72);
#endif
typedef void (*HsmpNativeProfileLogger)(const char* stage,uint32_t edge,const struct HsmpNativeProfileTrace*);
void hsmp_native_set_profile_logger(HsmpNativeProfileLogger logger);
void hsmp_native_profile_checkpoint(const char* stage,uint32_t edge);
void hsmp_native_profile_tick(uint32_t counter); // guard0, admission1, find2, PE3

// Copied full Outer hierarchy, never stored UObject pointers. Capture is tied
// to Rust's exact initial path lookup; verification uses original nodes only.
struct HsmpNativePathNode {uint64_t weak,address,name,class_weak,class_address,class_name;};
#ifdef __cplusplus
static_assert(sizeof(HsmpNativePathNode)==48);
#endif
typedef int32_t (*HsmpNativePathReader)(const struct HsmpNativePathNode*,uint32_t count,uint32_t capture,
    struct HsmpNativePathNode* output,uint32_t capacity,uint32_t* output_count,uint64_t* package_name,char* reason,uint32_t reason_capacity);
void hsmp_native_set_source_path_reader(HsmpNativePathReader reader);

#ifdef __cplusplus
}
#endif
